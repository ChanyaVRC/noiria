//! `noiria-export` の実処理。`noiria-tools` の bin `noiria-export`
//! （`src/bin/noiria_export.rs`）から呼ばれる。処理内容:
//!
//! 1. ゲームディレクトリの検証（`game.json` の必須項目、pak 非対応の `.mp4` 警告）
//! 2. OS 乱数でゲーム固有の pak 鍵を 32 バイト生成（`generate_key`）
//! 3. 全ターゲットのビルド済みバイナリを読み込み、`KEY_SLOT` マジックを
//!    バイト検索して鍵をバイナリパッチする（`patch_key_slot`）
//! 4. `crate::pack::pack_dir_with` で暗号化 pak を作る（出力先・ドットファイル・ゴミは除外）
//! 5. `out/<target>.tmp/{exe, game.pak, THIRD-PARTY.txt, README.txt}` に全部書いてから
//!    `out/<target>/` と入れ替える（途中で失敗しても旧出力は無傷。exe と pak の鍵が
//!    食い違う配布物を残さない）
//!
//! `docs/implementation-notes.md`「2026-07-12 配布対応 W7」参照。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use noiria_core::assets::GameConfig;
use noiria_core::pak;

use crate::pack::{pack_dir_with, ExcludedEntry, PackOptions, PackedEntry};
use crate::third_party::THIRD_PARTY_TXT;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Windows,
    Linux,
}

impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Target::Windows => "windows",
            Target::Linux => "linux",
        }
    }

    /// `target/dist/` 配下のビルド済みバイナリの既定ファイル名。
    fn default_bin_name(self) -> &'static str {
        match self {
            Target::Windows => "noiria.exe",
            Target::Linux => "noiria",
        }
    }

    /// 出力先の実行ファイル名（`exe_name` に拡張子を付けるかどうか）。
    fn out_bin_name(self, exe_name: &str) -> String {
        match self {
            Target::Windows => format!("{exe_name}.exe"),
            Target::Linux => exe_name.to_string(),
        }
    }
}

pub struct ExportOptions {
    pub game_dir: PathBuf,
    pub out_dir: PathBuf,
    pub targets: Vec<Target>,
    pub bin_windows: Option<PathBuf>,
    pub bin_linux: Option<PathBuf>,
    pub exe_name: Option<String>,
    /// W8: `.png` → WebP ロスレス自動変換（小さい方を採用）を行うかどうか。
    /// CLI の `--no-webp` で false にできる。
    pub webp: bool,
    /// W9: `.wav` → wav-flac (pak codec=2) 自動変換（小さい方を採用）を行うかどうか。
    /// CLI の `--no-flac` で false にできる。
    pub flac: bool,
    /// pak から外す glob（`noiria-export --exclude`、書式は `PackOptions::exclude`）。
    pub exclude: Vec<String>,
    /// false で `game.json` の `pack.exclude` を無視する（`--no-project-excludes`）。
    pub project_excludes: bool,
    /// false で既定除外（ドットファイル・`Thumbs.db` 等）を切る（`--no-default-excludes`）。
    pub default_excludes: bool,
}

pub struct GameCheck {
    pub title: String,
    pub id: String,
    /// pak 内から再生できない `.mp4` アセットの相対パス一覧（警告用、export は続行する）。
    pub mp4_warnings: Vec<String>,
}

pub struct TargetReport {
    pub target: Target,
    pub bin_path: PathBuf,
    pub bin_size: u64,
}

pub struct SkippedTargetReport {
    pub target: Target,
    pub bin_path: PathBuf,
}

pub struct ExportReport {
    pub exe_name: String,
    pub pack_entry_count: usize,
    pub pack_total_plain_len: u64,
    pub pack_total_pak_len: u64,
    pub mp4_warnings: Vec<String>,
    pub targets: Vec<TargetReport>,
    pub skipped_targets: Vec<SkippedTargetReport>,
    /// W8: WebP へ変換した PNG エントリ数と削減バイト数。
    pub webp_converted_count: usize,
    pub webp_bytes_saved: u64,
    /// W9: wav-flac へ変換した WAV エントリ数と削減バイト数。
    pub flac_converted_count: usize,
    pub flac_bytes_saved: u64,
    /// pak に入ったエントリ（`--list` 用）。
    pub pack_entries: Vec<PackedEntry>,
    /// 変換の検証失敗によるフォールバック等の警告。
    pub pack_warnings: Vec<String>,
    /// pak に入れなかったファイルと除外元（`--list-excluded` / 除外件数の表示用）。
    pub pack_excluded: Vec<ExcludedEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportPolicy {
    Strict,
    SkipMissingTargets,
}

/// `game.json` の必須項目確認 + pak 非対応 (`.mp4`) アセットの検出。
pub fn validate_game(game_dir: &Path) -> Result<GameCheck> {
    let config_path = game_dir.join("game.json");
    let text = fs::read_to_string(&config_path)
        .with_context(|| format!("game.json が読めません: {}", config_path.display()))?;
    let config: GameConfig = serde_json::from_str(&text)
        .with_context(|| format!("game.json のパースに失敗: {}", config_path.display()))?;
    if config.title.trim().is_empty() {
        bail!("game.json の title が空です");
    }
    if config.id.trim().is_empty() {
        bail!("game.json の id が空です");
    }

    let mut mp4_warnings = Vec::new();
    find_mp4(game_dir, game_dir, &mut mp4_warnings)?;

    Ok(GameCheck {
        title: config.title,
        id: config.id,
        mp4_warnings,
    })
}

fn find_mp4(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in
        fs::read_dir(dir).with_context(|| format!("ディレクトリが読めません: {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue; // リンクは pack 側が明示エラーにする
        }
        if file_type.is_dir() {
            find_mp4(root, &path, out)?;
            continue;
        }
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
        {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// ゲーム固有の pak 鍵を OS 乱数から生成する。外部クレートは使わず OS API を直接呼ぶ
/// （Windows: bcrypt.dll の `ProcessPrng` を FFI 宣言 / それ以外: `/dev/urandom`）。
pub fn generate_key() -> Result<[u8; 32]> {
    #[cfg(windows)]
    {
        windows_process_prng()
    }
    #[cfg(not(windows))]
    {
        read_dev_urandom()
    }
}

#[cfg(windows)]
fn windows_process_prng() -> Result<[u8; 32]> {
    // 初期化不要の軽量 API（Windows 8+）。`windows` クレートには依存せず宣言する。
    // 公式ドキュメントの宣言は `BOOL ProcessPrng(PBYTE pbData, SIZE_T cbData)`（BOOL = i32）で、
    // 実体は bcryptprimitives.dll。この DLL には import lib が無いので raw-dylib で結ぶ。
    #[link(name = "bcryptprimitives", kind = "raw-dylib")]
    extern "system" {
        fn ProcessPrng(pb_data: *mut u8, cb_data: usize) -> i32;
    }
    let mut buf = [0u8; 32];
    // SAFETY: `buf` は 32 バイトの有効な書き込み先で、`cb_data` は buf.len() と一致する。
    let ok = unsafe { ProcessPrng(buf.as_mut_ptr(), buf.len()) };
    if ok == 0 {
        bail!("ProcessPrng の呼び出しに失敗しました（Windows の乱数 API エラー）");
    }
    Ok(buf)
}

#[cfg(not(windows))]
fn read_dev_urandom() -> Result<[u8; 32]> {
    use std::io::Read;
    let mut f = fs::File::open("/dev/urandom").context("/dev/urandom が開けません")?;
    let mut buf = [0u8; 32];
    f.read_exact(&mut buf)
        .context("/dev/urandom から 32 バイト読み取れません")?;
    Ok(buf)
}

/// 実行ファイル中の `KEY_SLOT` マジックをバイト検索し、後続 32 バイトを鍵でパッチする。
/// マジックが 0 個・2 個以上見つかった場合は明確なエラーを返す（想定外バイナリの誤パッチ防止）。
pub fn patch_key_slot(binary: &mut [u8], key: &[u8; 32]) -> Result<()> {
    let magic = pak::key_slot_magic();
    let mut positions = Vec::new();
    if binary.len() >= magic.len() {
        for i in 0..=binary.len() - magic.len() {
            if binary[i..i + magic.len()] == magic {
                positions.push(i);
            }
        }
    }
    match positions.len() {
        0 => bail!(
            "KEY_SLOT マジックがバイナリ中に見つかりません（noiria-app 由来の実行ファイルではないか、\
             ビルド設定が想定外です）"
        ),
        1 => {
            let key_off = positions[0] + magic.len();
            if binary.len() < key_off + key.len() {
                bail!(
                    "KEY_SLOT の鍵領域が途中で切れています（32 バイトの領域が必要です）"
                );
            }
            binary[key_off..key_off + 32].copy_from_slice(key);
            Ok(())
        }
        n => bail!(
            "KEY_SLOT マジックが {n} 箇所見つかりました（1 箇所のみを想定しています。\
             バイナリが壊れているか、意図しない重複です）"
        ),
    }
}

/// exe/バイナリファイル名として安全な形へ正規化する（英数字・`-`・`_` のみ残し、
/// 空白は `_` に、それ以外の文字は除去。結果が空なら `"game"` にフォールバック）。
pub fn sanitize_exe_name(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if c.is_whitespace() && !out.is_empty() {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "game".to_string()
    } else {
        out
    }
}

fn readme_txt(title: &str, exe_name: &str, game_id: &str) -> String {
    format!(
        "{title} — 配布版\n\
         \n\
         ■ 起動方法\n\
         Windows: {exe_name}.exe をダブルクリックしてください。\n\
         Linux:   ./{exe_name} を実行してください\n\
         \x20        （実行権限が無い場合は先に `chmod +x {exe_name}` してください）。\n\
         \n\
         同じフォルダの game.pak は起動に必須です。実行ファイルと同じフォルダに\n\
         置いたまま配布してください。\n\
         \n\
         ■ アイコン\n\
         ウィンドウ/タスクバーのアイコンはゲームフォルダ直下の icon.png（あれば）を\n\
         使います。実行ファイル自体のアイコンはエンジン既定のままです。\n\
         \n\
         ■ ログ\n\
         問題が発生した場合、ログはセーブデータと同じ場所\n\
         （このゲーム専用のユーザーデータフォルダ、id={game_id}）の noiria.log に\n\
         記録されます。詳細度は環境変数 NOIRIA_LOG\n\
         （error/warn/info/debug/trace、既定 warn）で変更できます。\n\
         \n\
         ■ サードパーティ ライセンス\n\
         同梱の THIRD-PARTY.txt を参照してください。\n"
    )
}

fn default_bin_path(target: Target) -> PathBuf {
    PathBuf::from("target")
        .join("dist")
        .join(target.default_bin_name())
}

fn resolve_bin_path(opts: &ExportOptions, target: Target) -> PathBuf {
    match target {
        Target::Windows => opts
            .bin_windows
            .clone()
            .unwrap_or_else(|| default_bin_path(target)),
        Target::Linux => opts
            .bin_linux
            .clone()
            .unwrap_or_else(|| default_bin_path(target)),
    }
}

struct PreparedTarget {
    target: Target,
    bin_bytes: Vec<u8>,
}

fn preflight_targets(
    opts: &ExportOptions,
    key: &[u8; 32],
    policy: ExportPolicy,
) -> Result<(Vec<PreparedTarget>, Vec<SkippedTargetReport>)> {
    let mut prepared = Vec::new();
    let mut skipped = Vec::new();

    for &target in &opts.targets {
        let bin_path = resolve_bin_path(opts, target);
        let mut bin_bytes = match fs::read(&bin_path) {
            Ok(bytes) => bytes,
            Err(error)
                if policy == ExportPolicy::SkipMissingTargets
                    && error.kind() == std::io::ErrorKind::NotFound =>
            {
                skipped.push(SkippedTargetReport { target, bin_path });
                continue;
            }
            Err(error) => {
                return Err(anyhow::Error::new(error).context(format!(
                    "{} 向けのビルド済みバイナリが読めません: {}\
                     （先に `cargo build --profile dist --no-default-features --features engine-tsint,dist \
                     -p noiria-app` を実行するか、--bin-{} で明示的にパスを指定してください）",
                    target.name(),
                    bin_path.display(),
                    target.name(),
                )));
            }
        };

        patch_key_slot(&mut bin_bytes, key).with_context(|| {
            format!(
                "{} バイナリの KEY_SLOT パッチに失敗しました: {}",
                target.name(),
                bin_path.display()
            )
        })?;
        prepared.push(PreparedTarget { target, bin_bytes });
    }

    if prepared.is_empty() {
        bail!("出力可能なターゲットがありません（指定したバイナリがすべて見つかりません）");
    }

    Ok((prepared, skipped))
}

/// strict モードの export。全ターゲットを出力できない場合は成果物を書き始めない。
pub fn run_export(opts: &ExportOptions) -> Result<ExportReport> {
    run_export_with_policy(opts, ExportPolicy::Strict)
}

/// export 本体。policy に従って preflight した後、`out_dir` 配下へ配布物一式を書く。
pub fn run_export_with_policy(opts: &ExportOptions, policy: ExportPolicy) -> Result<ExportReport> {
    run_export_with_writer(opts, policy, &mut |path, bytes| fs::write(path, bytes))
}

/// [`run_export_with_policy`] のファイル書き込みを差し替え可能にしたもの（書き込み失敗を
/// 注入するテスト用。通常は使わない）。
#[doc(hidden)]
pub fn run_export_with_writer(
    opts: &ExportOptions,
    policy: ExportPolicy,
    write: &mut dyn FnMut(&Path, &[u8]) -> io::Result<()>,
) -> Result<ExportReport> {
    if opts.targets.is_empty() {
        bail!("--targets が空です（windows/linux の少なくとも一方を指定してください）");
    }

    let check = validate_game(&opts.game_dir)?;
    // mp4 警告は ExportReport::mp4_warnings で呼び出し側 (CLI) に返す（export 自体は続行する）。

    let exe_name = opts
        .exe_name
        .clone()
        .unwrap_or_else(|| sanitize_exe_name(&check.title));

    let key = generate_key().context("pak 鍵の生成に失敗しました")?;
    let (prepared_targets, skipped_targets) = preflight_targets(opts, &key, policy)?;

    // 出力先が game_dir 配下にあっても前回の出力を pak に詰めない。
    let mut pack_opts = PackOptions::new(opts.webp, opts.flac);
    pack_opts.exclude = opts.exclude.clone();
    pack_opts.default_excludes = opts.default_excludes;
    pack_opts.project_excludes = opts.project_excludes;
    pack_opts.skip_paths = vec![opts.out_dir.clone()];
    let packed = pack_dir_with(&opts.game_dir, &key, &pack_opts)
        .with_context(|| format!("pack に失敗しました: {}", opts.game_dir.display()))?;
    let (pak_bytes, pack_report) = (&packed.pak, &packed.report);

    let readme = readme_txt(&check.title, &exe_name, &check.id);

    // 1) 全ターゲットを `<target>.tmp` へ書く。ここで失敗しても既存の出力には触れていない。
    let mut stages: Vec<PathBuf> = Vec::new();
    let mut staged: Vec<(Target, u64, String)> = Vec::new();
    let stage_result = (|| -> Result<()> {
        for prepared in &prepared_targets {
            let target = prepared.target;
            let stage = opts.out_dir.join(format!("{}.tmp", target.name()));
            remove_path(&stage).with_context(|| {
                format!(
                    "前回の作業ディレクトリを削除できません: {}",
                    stage.display()
                )
            })?;
            fs::create_dir_all(&stage).with_context(|| {
                format!("出力ディレクトリを作成できません: {}", stage.display())
            })?;
            stages.push(stage.clone());

            let bin_name = target.out_bin_name(&exe_name);
            let bin_path = stage.join(&bin_name);
            write(&bin_path, &prepared.bin_bytes).with_context(|| {
                format!(
                    "実行ファイルの書き出しに失敗しました: {}",
                    bin_path.display()
                )
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(&bin_path) {
                    let mut perm = meta.permissions();
                    perm.set_mode(perm.mode() | 0o111);
                    let _ = fs::set_permissions(&bin_path, perm);
                }
            }
            for (name, bytes) in [
                ("game.pak", pak_bytes.as_slice()),
                ("THIRD-PARTY.txt", THIRD_PARTY_TXT.as_bytes()),
                ("README.txt", readme.as_bytes()),
            ] {
                write(&stage.join(name), bytes).with_context(|| {
                    format!("{name} の書き出しに失敗しました: {}", stage.display())
                })?;
            }
            staged.push((target, prepared.bin_bytes.len() as u64, bin_name));
        }
        Ok(())
    })();
    if let Err(e) = stage_result {
        for stage in &stages {
            let _ = remove_path(stage);
        }
        return Err(e);
    }

    // 2) 入れ替え。各ターゲットのディレクトリは exe・pak とも新しいものだけになる
    //    （旧名の exe は残らない）。失敗したターゲットは元に戻す。
    let mut targets = Vec::new();
    for ((target, bin_size, bin_name), stage) in staged.into_iter().zip(stages) {
        let final_dir = opts.out_dir.join(target.name());
        swap_dir(
            &stage,
            &final_dir,
            &opts.out_dir.join(format!("{}.old", target.name())),
        )?;
        targets.push(TargetReport {
            target,
            bin_path: final_dir.join(bin_name),
            bin_size,
        });
    }

    Ok(ExportReport {
        exe_name,
        pack_entry_count: pack_report.entry_count,
        pack_total_plain_len: pack_report.total_plain_len,
        pack_total_pak_len: pack_report.total_pak_len,
        mp4_warnings: check.mp4_warnings,
        targets,
        skipped_targets,
        webp_converted_count: pack_report.webp_converted_count,
        webp_bytes_saved: pack_report.webp_bytes_saved,
        flac_converted_count: pack_report.flac_converted_count,
        flac_bytes_saved: pack_report.flac_bytes_saved,
        pack_entries: packed.entries.clone(),
        pack_warnings: packed.warnings.clone(),
        pack_excluded: packed.excluded.clone(),
    })
}

/// ファイルでもディレクトリでも（無ければ何もせず）消す。
fn remove_path(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// `stage` を `final_dir` へ入れ替える。既存の `final_dir` は一旦 `backup` へ退避し、
/// 入れ替えに失敗したら戻す。退避の削除に失敗しても export 自体は成功（次回に掃除する）。
fn swap_dir(stage: &Path, final_dir: &Path, backup: &Path) -> Result<()> {
    remove_path(backup).with_context(|| {
        format!(
            "前回の退避ディレクトリを削除できません: {}",
            backup.display()
        )
    })?;
    let had_old = fs::symlink_metadata(final_dir).is_ok();
    if had_old {
        fs::rename(final_dir, backup).with_context(|| {
            format!(
                "既存の出力を退避できません（使用中の可能性があります）: {}",
                final_dir.display()
            )
        })?;
    }
    if let Err(e) = fs::rename(stage, final_dir) {
        if had_old {
            let _ = fs::rename(backup, final_dir);
        }
        let _ = remove_path(stage);
        return Err(anyhow::Error::new(e).context(format!(
            "出力の入れ替えに失敗しました: {}",
            final_dir.display()
        )));
    }
    if had_old {
        let _ = remove_path(backup);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_exe_name_keeps_ascii_and_replaces_space() {
        assert_eq!(sanitize_exe_name("My Game 2"), "My_Game_2");
        assert_eq!(sanitize_exe_name("Noiria サンプル"), "Noiria");
        assert_eq!(sanitize_exe_name("サンプル"), "game");
        assert_eq!(sanitize_exe_name(""), "game");
        assert_eq!(sanitize_exe_name("__weird--name__"), "weird--name");
    }

    #[test]
    fn generate_key_is_random_and_32_bytes() {
        let a = generate_key().unwrap();
        let b = generate_key().unwrap();
        assert_eq!(a.len(), 32);
        assert_ne!(
            a, b,
            "2 回連続で同じ鍵が出るのはほぼありえない（乱数実装のバグを疑う）"
        );
    }

    #[test]
    fn patch_key_slot_rejects_missing_and_duplicate_magic() {
        let key = [7u8; 32];

        let mut no_magic = vec![0u8; 100];
        assert!(patch_key_slot(&mut no_magic, &key).is_err());

        let magic = pak::key_slot_magic();
        let mut one = vec![0u8; 16];
        one.extend_from_slice(&magic);
        one.extend_from_slice(&[0u8; 32]);
        one.extend_from_slice(&[0u8; 16]);
        patch_key_slot(&mut one, &key).unwrap();
        assert_eq!(&one[16 + magic.len()..16 + magic.len() + 32], &key[..]);

        let mut truncated = vec![0u8; 8];
        truncated.extend_from_slice(&magic);
        truncated.extend_from_slice(&[0u8; 31]);
        assert!(patch_key_slot(&mut truncated, &key).is_err());

        let mut two = vec![0u8; 8];
        two.extend_from_slice(&magic);
        two.extend_from_slice(&[0u8; 32]);
        two.extend_from_slice(&magic);
        two.extend_from_slice(&[0u8; 32]);
        assert!(patch_key_slot(&mut two, &key).is_err());
    }
}
