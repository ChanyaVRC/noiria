//! ゲームディレクトリを NPK1 暗号化 pak にまとめる（`noiria-pack` CLI の実体）。
//! `noiria-export`（バイナリパッチ付き配布ビルド生成）からも同じロジックを使う。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use noiria_core::pak::{self, PackReport};

/// 既定で pak から外すゴミ（ドットファイル/ドットディレクトリは名前の先頭が `.` で判定）。
/// 名前は ASCII 大文字小文字を区別せず、ベース名に対して照合する。
const JUNK_NAMES: &[&str] = &["thumbs.db", "desktop.ini"];
const JUNK_SUFFIXES: &[&str] = &["~", ".bak", ".tmp"];

#[derive(Clone, Debug)]
pub struct PackOptions {
    /// 除外パターン（glob。`*` は `/` を跨がない、`**` は跨ぐ、`?` は 1 文字）。
    /// `/` を含まないパターンはベース名（ファイル名/ディレクトリ名）に、含むパターンは
    /// ゲームディレクトリからの相対パス全体に照合する。ディレクトリに一致すれば配下ごと除外。
    pub exclude: Vec<String>,
    /// `game.json` の `pack.exclude` を読んで除外に加える（`--no-project-excludes` で false）。
    pub project_excludes: bool,
    /// ドットファイル/ディレクトリと既知のゴミ（`Thumbs.db` `desktop.ini` `*~` `*.bak` `*.tmp`）を除外する。
    pub default_excludes: bool,
    /// 走査しないパス（出力先ディレクトリ・出力 pak 自身など）。未作成でもよい。
    pub skip_paths: Vec<PathBuf>,
    /// `.png` エントリを WebP ロスレスへ変換（元 PNG より小さく、かつデコードし直して
    /// 元と一致する場合だけ採用）。
    pub webp: bool,
    /// `.wav` エントリを wav-flac (pak codec=2) へ可逆変換（元 WAV より小さく、かつ
    /// デコードし直して元と一致する場合だけ採用）。
    pub flac: bool,
}

impl PackOptions {
    pub fn new(webp: bool, flac: bool) -> Self {
        Self {
            exclude: Vec::new(),
            project_excludes: true,
            default_excludes: true,
            skip_paths: Vec::new(),
            webp,
            flac,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PackedEntry {
    pub path: String,
    /// 変換前（ディスク上）のバイト数。
    pub plain_len: u64,
    /// `Some("webp")` / `Some("wav-flac")`: 変換を採用した。
    pub conversion: Option<&'static str>,
}

pub struct PackOutput {
    pub pak: Vec<u8>,
    pub report: PackReport,
    /// pak に入ったエントリ（キー順）。
    pub entries: Vec<PackedEntry>,
    /// エンコーダの出力が検証に通らず無変換へフォールバックした等の警告。
    pub warnings: Vec<String>,
    /// pak に入れなかったファイルと、その規則（パス順）。除外ディレクトリは配下のファイルを 1 件ずつ列挙する。
    pub excluded: Vec<ExcludedEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExcludeSource {
    /// `--out` / 出力 pak 自身。
    Output,
    Default,
    /// `game.json` の `pack.exclude`。
    Project,
    /// `--exclude`。
    Cli,
}

impl ExcludeSource {
    pub fn label(self) -> &'static str {
        match self {
            ExcludeSource::Output => "output",
            ExcludeSource::Default => "default",
            ExcludeSource::Project => "project",
            ExcludeSource::Cli => "cli",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExcludedEntry {
    pub path: String,
    pub source: ExcludeSource,
    /// 当たった規則（glob や既定規則名。`output` は空）。
    pub rule: String,
}

impl ExcludedEntry {
    /// `autotest/a.txt  (project: autotest/**)` の形。
    pub fn describe(&self) -> String {
        if self.rule.is_empty() {
            format!("{}  ({})", self.path, self.source.label())
        } else {
            format!("{}  ({}: {})", self.path, self.source.label(), self.rule)
        }
    }
}

/// `除外 3 件（default 0 / project 3 / cli 0 / output 0）` の 1 行要約。
pub fn excluded_summary(excluded: &[ExcludedEntry]) -> String {
    let n = |s: ExcludeSource| excluded.iter().filter(|e| e.source == s).count();
    format!(
        "除外 {} 件（default {} / project {} / cli {} / output {}）",
        excluded.len(),
        n(ExcludeSource::Default),
        n(ExcludeSource::Project),
        n(ExcludeSource::Cli),
        n(ExcludeSource::Output)
    )
}

/// `game_dir` 配下を再帰的に走査し、`(相対パス, 平文バイト列)` を集めて NPK1 pak を作る。
/// 相対パスは OS に関わらず常に `/` 区切り（`noiria_core::pak` の index キーと揃える）。
/// `exclude` は glob（[`PackOptions::exclude`]）。既定除外は有効（ドットファイル・ゴミ）。
/// 警告は stderr へ出す（構造化して受け取りたい場合は [`pack_dir_with`]）。
/// `webp` が true なら `.png` エントリを WebP ロスレスへ変換し、元 PNG より小さい場合
/// だけ差し替える（サイズ悪化ゼロ保証。パス・拡張子は `.png` のまま — エンジン側は
/// `assets::decode_image_bytes` のマジックバイト判定で振り分ける。W8、
/// `docs/implementation-notes.md` 2026-07-12 配布対応 W8 参照）。
/// `flac` が true なら `.wav` エントリを wav-flac (pak codec=2) へ可逆変換し、元 WAV より
/// 小さい場合だけ採用する（サイズ悪化ゼロ保証。読込時に `pak::Pak::read` が元 WAV バイト列を
/// 完全復元するので、シナリオ・`audio.rs` からは常に元の WAV に見える。W9、
/// `docs/implementation-notes.md` 2026-07-12 配布対応 W9 参照）。
/// どちらの変換も、採用前にデコードし直して元と一致することを確認する
/// （不一致なら変換しない。`docs/implementation-notes.md` 2026-11-20 R4）。
pub fn pack_dir(
    game_dir: &Path,
    key: &[u8; 32],
    exclude: &[String],
    webp: bool,
    flac: bool,
) -> Result<(Vec<u8>, PackReport)> {
    let mut opts = PackOptions::new(webp, flac);
    opts.exclude = exclude.to_vec();
    let out = pack_dir_with(game_dir, key, &opts)?;
    for w in &out.warnings {
        eprintln!("警告: {w}");
    }
    Ok((out.pak, out.report))
}

pub fn pack_dir_with(game_dir: &Path, key: &[u8; 32], opts: &PackOptions) -> Result<PackOutput> {
    pack_with_encoders(game_dir, key, opts, &Encoders::default())
}

/// 変換エンコーダ。テストで「デコードし直すと元と一致しない」エンコーダを差し込むために分離してある。
struct Encoders {
    webp: fn(u32, u32, &[u8]) -> Option<Vec<u8>>,
    flac: fn(&[u8]) -> Option<Vec<u8>>,
}

impl Default for Encoders {
    fn default() -> Self {
        Self {
            webp: |w, h, rgba| noiria_core::codec::webp::encode_lossless(w, h, rgba).ok(),
            flac: crate::flac_enc::try_encode,
        }
    }
}

fn pack_with_encoders(
    game_dir: &Path,
    key: &[u8; 32],
    opts: &PackOptions,
    enc: &Encoders,
) -> Result<PackOutput> {
    let project = read_project_config(game_dir, opts.project_excludes)?;
    let matcher = Matcher::new(game_dir, opts, &project.exclude)?;
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut excluded: Vec<ExcludedEntry> = Vec::new();
    walk(&matcher, &matcher.root, "", &mut files, &mut excluded)?;
    excluded.sort_by(|a, b| a.path.cmp(&b.path));
    check_protected(&excluded, &project.main)?;
    // 決定的な出力にするためパス順にソートしておく（entry_id の割当もこの順になる）。
    files.sort_by(|a, b| a.0.cmp(&b.0));
    check_duplicate_keys(files.iter().map(|(k, _)| k.as_str()))?;

    let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(files.len());
    let mut infos: Vec<PackedEntry> = Vec::with_capacity(files.len());
    for (key_path, path) in files {
        let bytes = std::fs::read(&path)
            .with_context(|| format!("ファイルが読めません: {}", path.display()))?;
        infos.push(PackedEntry {
            path: key_path.clone(),
            plain_len: bytes.len() as u64,
            conversion: None,
        });
        entries.push((key_path, bytes));
    }

    let mut warnings = Vec::new();
    let mut webp_converted_count = 0usize;
    let mut webp_bytes_saved = 0u64;
    if opts.webp {
        for ((path, bytes), info) in entries.iter_mut().zip(infos.iter_mut()) {
            if !path.to_ascii_lowercase().ends_with(".png") {
                continue;
            }
            match try_encode_webp(bytes, enc.webp) {
                WebpOutcome::Converted(webp_bytes) if webp_bytes.len() < bytes.len() => {
                    webp_bytes_saved += (bytes.len() - webp_bytes.len()) as u64;
                    webp_converted_count += 1;
                    info.conversion = Some("webp");
                    *bytes = webp_bytes;
                }
                WebpOutcome::Converted(_) | WebpOutcome::Unsupported => {}
                WebpOutcome::Mismatch => warnings.push(format!(
                    "{path}: WebP をデコードし直すと元の PNG と一致しないため、PNG のまま格納します"
                )),
            }
        }
    }

    let mut flac_paths: Vec<String> = Vec::new();
    let flac = opts.flac;
    let (pak_bytes, mut report) = pak::build_with_special_codec(entries, key, |path, data| {
        if !(flac && path.to_ascii_lowercase().ends_with(".wav")) {
            return None;
        }
        let blob = (enc.flac)(data)?;
        match noiria_core::codec::flac_subset::decode_wav(&blob) {
            Ok(back) if back == data => {
                flac_paths.push(path.to_string());
                Some((pak::CODEC_WAV_FLAC, blob))
            }
            _ => {
                warnings.push(format!(
                    "{path}: wav-flac をデコードし直すと元の WAV と一致しないため、通常の圧縮で格納します"
                ));
                None
            }
        }
    });
    for info in infos.iter_mut() {
        if flac_paths.contains(&info.path) {
            info.conversion = Some("wav-flac");
        }
    }
    report.webp_converted_count = webp_converted_count;
    report.webp_bytes_saved = webp_bytes_saved;
    Ok(PackOutput {
        pak: pak_bytes,
        report,
        entries: infos,
        warnings,
        excluded,
    })
}

enum WebpOutcome {
    Converted(Vec<u8>),
    /// 非対応 PNG 亜種などでデコード/エンコード自体が失敗した（pack 全体は失敗させない）。
    Unsupported,
    /// エンコードできたが、デコードし直した結果が元の画素と一致しない。
    Mismatch,
}

/// PNG バイト列を WebP ロスレスへ再エンコードし、デコードし直して画素が元と一致することを確認する。
fn try_encode_webp(
    png_bytes: &[u8],
    encode: fn(u32, u32, &[u8]) -> Option<Vec<u8>>,
) -> WebpOutcome {
    let Ok(png) = noiria_core::codec::png::decode(png_bytes) else {
        return WebpOutcome::Unsupported;
    };
    let Some(webp) = encode(png.width, png.height, &png.rgba) else {
        return WebpOutcome::Unsupported;
    };
    match noiria_core::codec::webp::decode_rgba(&webp) {
        Ok((w, h, rgba)) if (w, h) == (png.width, png.height) && rgba == png.rgba => {
            WebpOutcome::Converted(webp)
        }
        _ => WebpOutcome::Mismatch,
    }
}

/// ASCII の大文字小文字だけが違うキーは、Windows では共存できず Linux では別ファイルになる
/// ので、pak に入れると環境で挙動が変わる。エラーにする（完全一致の重複もここで捕まる）。
fn check_duplicate_keys<'a>(keys: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    for key in keys {
        if let Some(prev) = seen.insert(key.to_ascii_lowercase(), key) {
            bail!(
                "pak のキーが重複しています（大文字小文字だけが違う名前は Windows では共存できず \
                 Linux では別ファイルになります）: {prev} と {key}"
            );
        }
    }
    Ok(())
}

/// `game.json` から pack に関係する部分だけ読んだもの（エンジン側の `GameConfig` は未知の
/// `pack` フィールドを無視するので、ここで独立に解釈する）。
struct ProjectConfig {
    exclude: Vec<String>,
    main: String,
}

/// `game.json` が無ければ既定値（保護だけが空振りする）。`read_excludes` が false のときは
/// `pack.exclude` を読まず、壊れた game.json も無視する（`--no-project-excludes`）。
fn read_project_config(game_dir: &Path, read_excludes: bool) -> Result<ProjectConfig> {
    let mut cfg = ProjectConfig {
        exclude: Vec::new(),
        main: "main.ts".to_string(),
    };
    let path = game_dir.join("game.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(cfg);
    };
    match parse_project_config(&text) {
        Ok(parsed) => {
            cfg.main = parsed.main;
            if read_excludes {
                cfg.exclude = parsed.exclude;
            }
        }
        Err(e) if read_excludes => {
            return Err(e.context(format!(
                "game.json の pack 設定が不正です: {}",
                path.display()
            )))
        }
        Err(_) => {}
    }
    Ok(cfg)
}

fn parse_project_config(text: &str) -> Result<ProjectConfig> {
    let value: serde_json::Value =
        serde_json::from_str(text).context("game.json が JSON として読めません")?;
    let main = match value.get("main") {
        Some(serde_json::Value::String(s)) => s.clone(),
        _ => "main.ts".to_string(),
    };
    let mut exclude = Vec::new();
    match value.get("pack") {
        None => {}
        Some(serde_json::Value::Object(pack)) => match pack.get("exclude") {
            None => {}
            Some(serde_json::Value::Array(items)) => {
                for (i, item) in items.iter().enumerate() {
                    match item {
                        serde_json::Value::String(s) => exclude.push(s.clone()),
                        other => bail!(
                            "pack.exclude[{i}] は文字列でなければなりません（実際: {}）",
                            json_kind(other)
                        ),
                    }
                }
            }
            Some(other) => bail!(
                "pack.exclude は文字列の配列でなければなりません（実際: {}）",
                json_kind(other)
            ),
        },
        Some(other) => bail!(
            "pack はオブジェクト（{{\"exclude\": [...]}}）でなければなりません（実際: {}）",
            json_kind(other)
        ),
    }
    Ok(ProjectConfig { exclude, main })
}

fn json_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "真偽値",
        serde_json::Value::Number(_) => "数値",
        serde_json::Value::String(_) => "文字列",
        serde_json::Value::Array(_) => "配列",
        serde_json::Value::Object(_) => "オブジェクト",
    }
}

/// `game.json` と、`main` が指すファイルは除外させない（起動できない pak を作らない）。
/// `main` が実在しない場合は検査しない（除外されたファイルの一覧にあるときだけエラー）。
fn check_protected(excluded: &[ExcludedEntry], main: &str) -> Result<()> {
    let main = normalize_pattern(main);
    for (what, name) in [
        ("game.json", "game.json"),
        ("main（game.json の \"main\"）", main.as_str()),
    ] {
        if let Some(e) = excluded.iter().find(|e| e.path == name) {
            bail!(
                "{what} が pak から除外されます（{}）。除外規則を見直してください: {name}",
                e.describe()
            );
        }
    }
    Ok(())
}

struct Matcher {
    /// 正規化（canonicalize）済みの走査ルート。
    root: PathBuf,
    skip: Vec<PathBuf>,
    /// (正規化した glob, 由来, 元の記述)。判定は既定 → project → cli の順。
    patterns: Vec<(String, ExcludeSource, String)>,
    default_excludes: bool,
}

impl Matcher {
    fn new(game_dir: &Path, opts: &PackOptions, project: &[String]) -> Result<Self> {
        let root = std::fs::canonicalize(game_dir)
            .with_context(|| format!("ゲームディレクトリが開けません: {}", game_dir.display()))?;
        let mut patterns = Vec::new();
        for (list, source) in [
            (project, ExcludeSource::Project),
            (opts.exclude.as_slice(), ExcludeSource::Cli),
        ] {
            for p in list {
                patterns.push((normalize_pattern(p), source, p.clone()));
            }
        }
        Ok(Self {
            root,
            skip: opts
                .skip_paths
                .iter()
                .map(|p| canonicalize_lossy(p))
                .collect(),
            patterns,
            default_excludes: opts.default_excludes,
        })
    }

    fn exclusion(&self, rel: &str, name: &str, path: &Path) -> Option<(ExcludeSource, String)> {
        if self.skip.iter().any(|s| s == path) {
            return Some((ExcludeSource::Output, String::new()));
        }
        if self.default_excludes {
            if let Some(rule) = default_exclude_rule(name) {
                return Some((ExcludeSource::Default, rule));
            }
        }
        self.patterns.iter().find_map(|(pat, source, original)| {
            let hit = if pat.contains('/') {
                glob_match(pat, rel)
            } else {
                glob_match(pat, name)
            };
            hit.then(|| (*source, original.clone()))
        })
    }
}

fn normalize_pattern(pat: &str) -> String {
    let p = pat.replace('\\', "/");
    p.trim_start_matches("./").trim_end_matches('/').to_string()
}

/// 既定除外に当たったとき、その規則名（ドットファイルは `.*`）。
fn default_exclude_rule(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    if name.starts_with('.') {
        return Some(".*".to_string());
    }
    if JUNK_NAMES.contains(&lower.as_str()) {
        return Some(name.to_string());
    }
    JUNK_SUFFIXES
        .iter()
        .find(|s| lower.ends_with(*s))
        .map(|s| format!("*{s}"))
}

/// 存在しないパス（これから作る出力先）も、存在する最も近い祖先を正規化して解決する。
fn canonicalize_lossy(path: &Path) -> PathBuf {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p;
    }
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match (abs.parent(), abs.file_name()) {
        (Some(parent), Some(name)) => canonicalize_lossy(parent).join(name),
        _ => abs,
    }
}

/// `*`（`/` を跨がない任意長）・`**`（`/` を跨ぐ）・`?`（`/` 以外の 1 文字）だけの glob。
fn glob_match(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') => {
                let double = p.get(1) == Some(&'*');
                let rest = &p[if double { 2 } else { 1 }..];
                (0..=t.len()).any(|n| go(rest, &t[n..]) && (double || !t[..n].contains(&'/')))
            }
            Some('?') => t.first().is_some_and(|c| *c != '/') && go(&p[1..], &t[1..]),
            Some(c) => t.first() == Some(c) && go(&p[1..], &t[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    go(&p, &t)
}

fn walk(
    m: &Matcher,
    dir: &Path,
    prefix: &str,
    out: &mut Vec<(String, PathBuf)>,
    excluded: &mut Vec<ExcludedEntry>,
) -> Result<()> {
    let read_dir = std::fs::read_dir(dir)
        .with_context(|| format!("ディレクトリが読めません: {}", dir.display()))?;
    for entry in read_dir {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let lossy = file_name.to_string_lossy();
        let rel = if prefix.is_empty() {
            lossy.to_string()
        } else {
            format!("{prefix}/{lossy}")
        };
        if let Some((source, rule)) = m.exclusion(&rel, &lossy, &path) {
            if source == ExcludeSource::Output {
                excluded.push(ExcludedEntry {
                    path: rel,
                    source,
                    rule,
                });
            } else {
                list_excluded(&path, &rel, source, &rule, excluded);
            }
            continue;
        }
        if file_name.to_str().is_none() {
            bail!("ファイル名が UTF-8 ではありません（pak のキーにできません）: {rel}");
        }
        let file_type = entry
            .file_type()
            .with_context(|| format!("ファイル種別が取得できません: {rel}"))?;
        if file_type.is_symlink() {
            bail!(
                "シンボリックリンク/ジャンクションは pak に含められません（ゲームディレクトリ外を \
                 指す・循環する恐れがあります。実体を置くか --exclude で除外してください）: {rel}"
            );
        } else if file_type.is_dir() {
            walk(m, &path, &rel, out, excluded)?;
        } else if file_type.is_file() {
            out.push((rel, path));
        } else {
            bail!("通常のファイルでもディレクトリでもありません: {rel}");
        }
    }
    Ok(())
}

/// 除外された `path`（ファイルならそれ自身、ディレクトリなら配下のファイル）を報告用に列挙する。
/// 読めない箇所は黙って飛ばす（報告だけの処理で pack を失敗させない）。
fn list_excluded(
    path: &Path,
    rel: &str,
    source: ExcludeSource,
    rule: &str,
    excluded: &mut Vec<ExcludedEntry>,
) {
    let is_dir = std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());
    if !is_dir {
        excluded.push(ExcludedEntry {
            path: rel.to_string(),
            source,
            rule: rule.to_string(),
        });
        return;
    }
    let Ok(read_dir) = std::fs::read_dir(path) else {
        return;
    };
    for entry in read_dir.flatten() {
        let child = format!("{rel}/{}", entry.file_name().to_string_lossy());
        list_excluded(&entry.path(), &child, source, rule, excluded);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRoot;

    fn write(root: &TestRoot, rel: &str, content: &[u8]) {
        root.write(rel, content);
    }

    #[test]
    fn pack_dir_roundtrips_and_excludes() {
        let root = TestRoot::new("pack_dir");
        write(&root, "game.json", br#"{"title":"t"}"#);
        write(&root, "scenario/first.ks", b"@title\nhello");
        write(&root, "secret/ignore_me.txt", b"should be excluded");

        let key = [1u8; 32];
        let (bytes, report) =
            pack_dir(root.path(), &key, &["secret".to_string()], true, true).unwrap();
        assert_eq!(report.entry_count, 2);

        let pak = noiria_core::pak::Pak::open(bytes, &key).unwrap();
        assert!(pak.exists("game.json"));
        assert!(pak.exists("scenario/first.ks"));
        assert!(!pak.exists("secret/ignore_me.txt"));
    }

    #[test]
    fn glob_patterns() {
        assert!(glob_match("*.txt", "a.txt"));
        assert!(!glob_match("*.txt", "dir/a.txt"), "* は / を跨がない");
        assert!(glob_match("**/*.txt", "dir/sub/a.txt"));
        assert!(glob_match("autotest/**", "autotest/x/y.txt"));
        assert!(glob_match("a?c", "abc") && !glob_match("a?c", "a/c"));
        assert!(glob_match("secret", "secret") && !glob_match("secret", "secret2"));
    }

    #[test]
    fn exclude_globs_match_names_and_paths() {
        let root = TestRoot::new("pack_glob");
        write(&root, "game.json", b"{}");
        write(&root, "a/notes.md", b"x");
        write(&root, "b/notes.md", b"x");
        write(&root, "autotest/one.txt", b"x");
        write(&root, "keep/one.txt", b"x");
        let mut opts = PackOptions::new(false, false);
        opts.exclude = vec!["*.md".into(), "autotest/".into(), "keep/two*".into()];
        let out = pack_dir_with(root.path(), &[0u8; 32], &opts).unwrap();
        let names: Vec<&str> = out.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, ["game.json", "keep/one.txt"]);
    }

    #[test]
    fn skip_paths_exclude_output_inside_game_dir_even_when_not_created_yet() {
        let root = TestRoot::new("pack_skip");
        write(&root, "game.json", b"{}");
        write(&root, "export/windows/game.pak", b"old pak");
        write(&root, "out.pak", b"old pak file");
        let mut opts = PackOptions::new(false, false);
        opts.skip_paths = vec![
            root.path().join("export"),
            root.path().join("out.pak"),
            root.path().join("not-yet").join("dir"),
        ];
        let out = pack_dir_with(root.path(), &[0u8; 32], &opts).unwrap();
        let names: Vec<&str> = out.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, ["game.json"]);
    }

    #[test]
    fn default_excludes_can_be_disabled() {
        let root = TestRoot::new("pack_nodefault");
        write(&root, "game.json", b"{}");
        write(&root, ".hidden/x", b"x");
        write(&root, "Thumbs.db", b"x");
        let mut opts = PackOptions::new(false, false);
        opts.default_excludes = false;
        let out = pack_dir_with(root.path(), &[0u8; 32], &opts).unwrap();
        assert_eq!(out.entries.len(), 3);
    }

    #[test]
    fn duplicate_keys_differing_only_in_ascii_case_are_rejected() {
        let err =
            check_duplicate_keys(["assets/Room.png", "assets/room.png"].into_iter()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("Room.png") && msg.contains("room.png"));
        assert!(check_duplicate_keys(["a/b", "a/c", "a/b2"].into_iter()).is_ok());
    }

    #[test]
    fn entries_list_reports_paths_in_key_order() {
        let root = TestRoot::new("pack_entries");
        write(&root, "z.txt", b"1");
        write(&root, "a/b.txt", b"22");
        let out = pack_dir_with(root.path(), &[0u8; 32], &PackOptions::new(false, false)).unwrap();
        let listed: Vec<(&str, u64)> = out
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e.plain_len))
            .collect();
        assert_eq!(listed, [("a/b.txt", 2), ("z.txt", 1)]);
    }

    /// 圧縮に適した合成 PNG（大きな単色領域）を作る。WebP ロスレスの方が
    /// 確実に小さくなる素材（自作 PNG エンコーダは stored ブロックのみのため）。
    fn compressible_png(w: u32, h: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                if (x / 16 + y / 16) % 2 == 0 {
                    rgba.extend_from_slice(&[200, 60, 60, 255]);
                } else {
                    rgba.extend_from_slice(&[60, 60, 200, 255]);
                }
            }
        }
        noiria_core::codec::png::encode(w, h, &rgba)
    }

    #[test]
    fn pack_dir_converts_png_to_webp_when_smaller_and_engine_reads_it_back() {
        let root = TestRoot::new("pack_webp");
        write(&root, "game.json", br#"{"title":"t","id":"webp-test"}"#);
        let png_bytes = compressible_png(256, 256);
        write(&root, "assets/bg/room.png", &png_bytes);

        let key = noiria_core::pak::dev_key();
        let (pak_bytes, report) = pack_dir(root.path(), &key, &[], true, false).unwrap();
        assert_eq!(
            report.webp_converted_count, 1,
            "圧縮に適した PNG は WebP の方が小さくなるはず"
        );
        assert!(report.webp_bytes_saved > 0);

        // pak 内バイト列が実際に WebP (RIFF/WEBP) に差し替わっていること（拡張子は .png のまま）。
        let pak = noiria_core::pak::Pak::open(pak_bytes.clone(), &key).unwrap();
        let stored = pak.read("assets/bg/room.png").unwrap();
        assert_eq!(&stored[0..4], b"RIFF");
        assert_eq!(&stored[8..12], b"WEBP");
        assert!(
            stored.len() < png_bytes.len(),
            "pak 内サイズは元 PNG より小さいはず"
        );

        // エンジン側 (GameDir) がマジックバイト判定で正しく WebP をデコードし、
        // 元 PNG をデコードした結果とピクセルが完全一致すること。
        let out = TestRoot::new("pack_webp_out");
        let pak_path = out.write("game.pak", &pak_bytes);
        let (game_dir, _config) = noiria_core::assets::GameDir::open(&pak_path).unwrap();
        let (w, h, rgba) = game_dir.load_image_rgba("assets/bg/room.png").unwrap();
        let original = noiria_core::codec::png::decode(&png_bytes).unwrap();
        assert_eq!((w, h), (original.width, original.height));
        assert_eq!(rgba, original.rgba);
    }

    #[test]
    fn pack_dir_no_webp_keeps_original_png_bytes() {
        let root = TestRoot::new("pack_no_webp");
        write(&root, "game.json", br#"{"title":"t","id":"no-webp-test"}"#);
        let png_bytes = compressible_png(256, 256);
        write(&root, "assets/bg/room.png", &png_bytes);

        let key = [3u8; 32];
        let (pak_bytes, report) = pack_dir(root.path(), &key, &[], false, false).unwrap();
        assert_eq!(report.webp_converted_count, 0);
        assert_eq!(report.webp_bytes_saved, 0);

        let pak = noiria_core::pak::Pak::open(pak_bytes, &key).unwrap();
        let stored = pak.read("assets/bg/room.png").unwrap();
        assert_eq!(
            stored, png_bytes,
            "--no-webp 相当時は元 PNG バイト列のまま格納されるはず"
        );
    }

    #[test]
    fn webp_encoder_that_does_not_roundtrip_falls_back_to_png_with_warning() {
        let root = TestRoot::new("pack_webp_bad_enc");
        write(&root, "game.json", b"{}");
        let png_bytes = compressible_png(64, 64);
        write(&root, "room.png", &png_bytes);

        // 画素を 1 つ壊した（= デコーダと食い違う）WebP を返すエンコーダ。
        let corrupting = Encoders {
            webp: |w, h, rgba| {
                let mut bad = rgba.to_vec();
                bad[0] ^= 1;
                noiria_core::codec::webp::encode_lossless(w, h, &bad).ok()
            },
            ..Encoders::default()
        };
        let key = [4u8; 32];
        let out = pack_with_encoders(
            root.path(),
            &key,
            &PackOptions::new(true, false),
            &corrupting,
        )
        .unwrap();
        assert_eq!(out.report.webp_converted_count, 0);
        assert!(out.warnings.iter().any(|w| w.contains("room.png")));
        let pak = noiria_core::pak::Pak::open(out.pak, &key).unwrap();
        assert_eq!(pak.read("room.png").unwrap(), png_bytes);
    }

    /// wav.rs のテストと同型の最小 WAV バイト列組み立てヘルパー（16bit PCM 固定）。
    fn build_wav16(channels: u16, rate: u32, body: &[u8]) -> Vec<u8> {
        let block_align = channels * 2;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + body.len() as u32).to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * block_align as u32).to_le_bytes());
        v.extend_from_slice(&block_align.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    /// 圧縮に適した WAV（無音）を作る: 16bit mono 4 秒ぶん。
    fn compressible_wav() -> Vec<u8> {
        let body = vec![0u8; 44100 * 2 * 4];
        build_wav16(1, 44100, &body)
    }

    #[test]
    fn pack_dir_converts_wav_to_flac_when_smaller_and_engine_reads_it_back() {
        let root = TestRoot::new("pack_flac");
        write(&root, "game.json", br#"{"title":"t","id":"flac-test"}"#);
        let wav_bytes = compressible_wav();
        write(&root, "assets/bgm/main.wav", &wav_bytes);

        let key = noiria_core::pak::dev_key();
        let (pak_bytes, report) = pack_dir(root.path(), &key, &[], false, true).unwrap();
        assert_eq!(
            report.flac_converted_count, 1,
            "無音 WAV は wav-flac の方が確実に小さくなるはず"
        );
        assert!(report.flac_bytes_saved > 0);

        let pak = noiria_core::pak::Pak::open(pak_bytes.clone(), &key).unwrap();
        assert_eq!(
            pak.codec_of("assets/bgm/main.wav"),
            Some(noiria_core::pak::CODEC_WAV_FLAC)
        );
        // read_bytes（エンジンが実際に使う API）は常に元 WAV バイト列を完全復元する。
        let stored = pak.read("assets/bgm/main.wav").unwrap();
        assert_eq!(
            stored, wav_bytes,
            "pak 経由の読込結果は元 WAV バイト列と完全一致するはず"
        );

        let out = TestRoot::new("pack_flac_out");
        let pak_path = out.write("game.pak", &pak_bytes);
        let (game_dir, _config) = noiria_core::assets::GameDir::open(&pak_path).unwrap();
        assert_eq!(
            game_dir.read_bytes("assets/bgm/main.wav").unwrap(),
            wav_bytes
        );
    }

    #[test]
    fn pack_dir_no_flac_keeps_wav_uncompressed_by_flac() {
        let root = TestRoot::new("pack_no_flac");
        write(&root, "game.json", br#"{"title":"t","id":"no-flac-test"}"#);
        let wav_bytes = compressible_wav();
        write(&root, "assets/bgm/main.wav", &wav_bytes);

        let key = [5u8; 32];
        let (pak_bytes, report) = pack_dir(root.path(), &key, &[], false, false).unwrap();
        assert_eq!(report.flac_converted_count, 0);
        assert_eq!(report.flac_bytes_saved, 0);

        let pak = noiria_core::pak::Pak::open(pak_bytes, &key).unwrap();
        assert_ne!(
            pak.codec_of("assets/bgm/main.wav"),
            Some(noiria_core::pak::CODEC_WAV_FLAC)
        );
        assert_eq!(pak.read("assets/bgm/main.wav").unwrap(), wav_bytes);
    }

    #[test]
    fn flac_encoder_that_does_not_roundtrip_falls_back_with_warning() {
        let root = TestRoot::new("pack_flac_bad_enc");
        write(&root, "game.json", b"{}");
        let wav_bytes = compressible_wav();
        write(&root, "bgm/main.wav", &wav_bytes);

        // 有効な wav-flac だが元と別の WAV にデコードされる（= 可逆でない）エンコーダ。
        let lossy = Encoders {
            flac: |wav| {
                let mut other = wav.to_vec();
                let last = other.len() - 1;
                other[last] ^= 1;
                crate::flac_enc::try_encode(&other)
            },
            ..Encoders::default()
        };
        let key = [6u8; 32];
        let out =
            pack_with_encoders(root.path(), &key, &PackOptions::new(false, true), &lossy).unwrap();
        assert_eq!(out.report.flac_converted_count, 0);
        assert!(out.warnings.iter().any(|w| w.contains("main.wav")));
        assert!(out.entries.iter().all(|e| e.conversion.is_none()));
        let pak = noiria_core::pak::Pak::open(out.pak, &key).unwrap();
        assert_ne!(
            pak.codec_of("bgm/main.wav"),
            Some(noiria_core::pak::CODEC_WAV_FLAC)
        );
        assert_eq!(pak.read("bgm/main.wav").unwrap(), wav_bytes);
    }

    fn game_json_with_pack(pack: &str) -> String {
        format!(r#"{{"title":"t","id":"t","main":"main.ts","pack":{pack}}}"#)
    }

    fn names(out: &PackOutput) -> Vec<&str> {
        out.entries.iter().map(|e| e.path.as_str()).collect()
    }

    #[test]
    fn game_json_pack_exclude_is_applied_and_reported_with_its_rule() {
        let root = TestRoot::new("pack_project");
        write(
            &root,
            "game.json",
            game_json_with_pack(r#"{"exclude":["autotest/**"]}"#).as_bytes(),
        );
        write(&root, "main.ts", b"x");
        write(&root, "autotest/a.txt", b"x");
        write(&root, "autotest/sub/b.txt", b"x");
        let out = pack_dir_with(root.path(), &[0u8; 32], &PackOptions::new(false, false)).unwrap();
        assert_eq!(names(&out), ["game.json", "main.ts"]);
        let ex: Vec<String> = out.excluded.iter().map(|e| e.describe()).collect();
        assert_eq!(
            ex,
            [
                "autotest/a.txt  (project: autotest/**)",
                "autotest/sub/b.txt  (project: autotest/**)"
            ]
        );
        assert_eq!(
            excluded_summary(&out.excluded),
            "除外 2 件（default 0 / project 2 / cli 0 / output 0）"
        );
    }

    #[test]
    fn project_cli_and_default_excludes_are_unioned_and_each_can_be_switched_off() {
        let root = TestRoot::new("pack_union");
        write(
            &root,
            "game.json",
            game_json_with_pack(r#"{"exclude":["*.md"]}"#).as_bytes(),
        );
        write(&root, "main.ts", b"x");
        write(&root, "notes.md", b"x");
        write(&root, "draft.txt", b"x");
        write(&root, "Thumbs.db", b"x");
        let mut opts = PackOptions::new(false, false);
        opts.exclude = vec!["draft.txt".into()];
        let out = pack_dir_with(root.path(), &[0u8; 32], &opts).unwrap();
        assert_eq!(names(&out), ["game.json", "main.ts"]);
        assert_eq!(
            excluded_summary(&out.excluded),
            "除外 3 件（default 1 / project 1 / cli 1 / output 0）"
        );

        // --no-default-excludes は既定だけを切る（game.json の指定は有効のまま）。
        opts.default_excludes = false;
        let out = pack_dir_with(root.path(), &[0u8; 32], &opts).unwrap();
        assert_eq!(names(&out), ["Thumbs.db", "game.json", "main.ts"]);

        // --no-project-excludes は game.json の指定だけを切る。
        opts.default_excludes = true;
        opts.project_excludes = false;
        let out = pack_dir_with(root.path(), &[0u8; 32], &opts).unwrap();
        assert_eq!(names(&out), ["game.json", "main.ts", "notes.md"]);
    }

    #[test]
    fn malformed_pack_settings_name_the_offending_part() {
        for (pack, needle) in [
            (r#"[]"#, "pack はオブジェクト"),
            (r#"{"exclude":"autotest"}"#, "pack.exclude は文字列の配列"),
            (r#"{"exclude":["ok", 3]}"#, "pack.exclude[1] は文字列"),
        ] {
            let root = TestRoot::new("pack_bad_type");
            write(&root, "game.json", game_json_with_pack(pack).as_bytes());
            let err = pack_dir_with(root.path(), &[0u8; 32], &PackOptions::new(false, false))
                .err()
                .expect("不正な型は error");
            assert!(format!("{err:#}").contains(needle), "{pack}: {err:#}");

            // --no-project-excludes なら pack 設定は読まないので通る。
            let mut opts = PackOptions::new(false, false);
            opts.project_excludes = false;
            pack_dir_with(root.path(), &[0u8; 32], &opts).unwrap();
        }
    }

    #[test]
    fn game_json_and_main_cannot_be_excluded() {
        let root = TestRoot::new("pack_protected");
        write(
            &root,
            "game.json",
            game_json_with_pack(r#"{"exclude":["*.json"]}"#).as_bytes(),
        );
        write(&root, "main.ts", b"x");
        let err = pack_dir_with(root.path(), &[0u8; 32], &PackOptions::new(false, false))
            .err()
            .expect("game.json の除外は error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("game.json") && msg.contains("project: *.json"),
            "{msg}"
        );

        // CLI の --exclude でも同じ。main（game.json の "main" が指すファイル）も保護。
        let root = TestRoot::new("pack_protected_main");
        write(&root, "game.json", game_json_with_pack("{}").as_bytes());
        write(&root, "main.ts", b"x");
        let mut opts = PackOptions::new(false, false);
        opts.exclude = vec!["main.ts".into()];
        let err = pack_dir_with(root.path(), &[0u8; 32], &opts)
            .err()
            .expect("main の除外は error");
        assert!(format!("{err:#}").contains("main"), "{err:#}");

        // ディレクトリ配下の main も、ディレクトリごと除外されれば error。
        let root = TestRoot::new("pack_protected_main_dir");
        write(
            &root,
            "game.json",
            br#"{"title":"t","id":"t","main":"src/main.ts"}"#,
        );
        write(&root, "src/main.ts", b"x");
        let mut opts = PackOptions::new(false, false);
        opts.exclude = vec!["src".into()];
        assert!(pack_dir_with(root.path(), &[0u8; 32], &opts).is_err());

        // main が実在しないディレクトリでは検査しない（従来どおり pack できる）。
        let root = TestRoot::new("pack_no_main_file");
        write(&root, "game.json", game_json_with_pack("{}").as_bytes());
        write(&root, "a.txt", b"x");
        pack_dir_with(root.path(), &[0u8; 32], &PackOptions::new(false, false)).unwrap();
    }

    #[test]
    fn engine_game_config_ignores_the_pack_field() {
        let cfg: noiria_core::assets::GameConfig =
            serde_json::from_str(&game_json_with_pack(r#"{"exclude":["autotest/**"]}"#)).unwrap();
        assert_eq!(cfg.main, "main.ts");
    }
}
