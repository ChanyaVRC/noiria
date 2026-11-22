//! ゲームディレクトリ配下に閉じたアセット解決と画像読み込み。
//! 画像デコードは自作コーデック (`codec::png` / `codec::jpeg`) と、1 モジュール
//! 隔離した `codec::webp`（外部クレート `image-webp` を包む、W8）に閉じる。
//!
//! すべてのゲーム資産読込（画像/フォント/音声/動画/V8・tsint モジュール）はこの
//! `GameDir` の 1 点に集約する。内部は `Backend` enum で抽象化されており、
//! `Backend::Dir`（実ディレクトリ）と `Backend::Pak`（W6 で追加した NPK1 暗号化 pak、
//! `crate::pak`）の 2 種類がある（呼び出し側から絶対パスの受け渡しを撲滅するのが
//! 狙い。`docs/implementation-notes.md`「2026-07-12 配布対応 W5」「同 W6」参照）。

use std::collections::HashSet;
use std::fs;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::codec::video::VideoSource;
use crate::pak::{OwnedPakEntryReader, Pak, CODEC_STORED};
use crate::pixmap::Pixmap;

/// `GameDir::open_reader` が返すシーク可能なアセットリーダー。
/// 実ファイル・pak 内無圧縮エントリはディスクから逐次読み出し、pak 内圧縮エントリ
/// (DEFLATE / wav-flac) のみ展開済みメモリバッファへフォールバックする。
pub enum AssetReader {
    File { file: fs::File, len: u64 },
    Pak(OwnedPakEntryReader),
    Memory(Cursor<Vec<u8>>),
}

impl AssetReader {
    pub fn len(&self) -> u64 {
        match self {
            Self::File { len, .. } => *len,
            Self::Pak(r) => r.len(),
            Self::Memory(c) => c.get_ref().len() as u64,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Read for AssetReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::File { file, .. } => file.read(buf),
            Self::Pak(r) => r.read(buf),
            Self::Memory(c) => c.read(buf),
        }
    }
}

impl Seek for AssetReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::File { file, .. } => file.seek(pos),
            Self::Pak(r) => r.seek(pos),
            Self::Memory(c) => c.seek(pos),
        }
    }
}

fn default_width() -> u32 {
    1280
}
fn default_height() -> u32 {
    720
}
fn default_main() -> String {
    "main.ts".to_string()
}
// 開発ビルドは v8 が既定。配布ビルド（W7 `dist` feature）は tsint が既定
// （軽量な自作 TS インタプリタのみを同梱する運用のため）。
#[cfg(not(feature = "dist"))]
fn default_engine() -> String {
    "v8".to_string()
}
#[cfg(feature = "dist")]
fn default_engine() -> String {
    "tsint".to_string()
}

/// games/<name>/game.json
#[derive(Clone, Debug, Deserialize)]
pub struct GameConfig {
    pub title: String,
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default = "default_main")]
    pub main: String,
    /// セーブディレクトリ名に使う一意な ID
    pub id: String,
    /// スクリプトエンジン: `"v8"`（既定、`deno_core`）または `"tsint"`（完全自作 TS
    /// インタプリタ、別リポジトリ `tsint`）。`noiria <game> --engine <name>` の
    /// CLI 引数で上書きできる（`main.rs` 参照）。
    #[serde(default = "default_engine")]
    pub engine: String,
}

/// ゲーム資産の実体を保持する場所。`Dir` は従来通りの実ディレクトリ、`Pak` は
/// W6 で追加した NPK1 暗号化 pak（`crate::pak::Pak`）。`GameDir` の公開 API は
/// 両方の分岐で共通のまま（呼び出し側は Backend の違いを意識しない）。
enum Backend {
    Dir(PathBuf),
    /// `pseudo_root` は pak ファイル自身の実パス。実ディレクトリではないが、
    /// V8/tsint モジュールローダが `file://` 基点 URL を組み立てるためだけに
    /// `GameDir::root()` 経由で使う（実ファイルアクセスには使われない。
    /// `docs/implementation-notes.md` 2026-07-12 W6 参照）。
    Pak {
        pak: Pak,
        pseudo_root: PathBuf,
    },
}

#[derive(Clone)]
pub struct GameDir {
    backend: Arc<Backend>,
}

/// ルート外への脱出 (`..` / 絶対パス / ドライブ文字等) を拒否する共通ジェイルチェック。
/// `Backend::Dir` の `resolve()`・`Backend::Pak` の `pak_key()` の双方から呼ぶ
/// （W6 で Pak バックエンドを追加する際に factor out した。判定ロジック自体は無変更）。
fn check_jail(rel: &str) -> Result<()> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        bail!("絶対パスは指定できません: {rel}");
    }
    for comp in rel_path.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            _ => bail!("ゲームディレクトリ外のパスは指定できません: {rel}"),
        }
    }
    Ok(())
}

/// `rel` を pak のインデックスキー形式（`/` 区切り、`.`/空要素を除去）に正規化する。
/// `noiria-tools::pack::pack_dir` が pak を作る際も常に `/` 区切りでパスを組み立てるため、
/// ここでも OS のパス区切りに関わらず `/` 区切りへ揃える（Windows の `\` はもちろん、
/// Linux 上でバックスラッシュ付きの `rel` が渡されても同様に分解する）。
fn pak_key(rel: &str) -> Result<String> {
    check_jail(rel)?;
    let mut parts: Vec<String> = Vec::new();
    for comp in Path::new(rel).components() {
        if let Component::Normal(os) = comp {
            for piece in os.to_string_lossy().split('\\') {
                if !piece.is_empty() {
                    parts.push(piece.to_string());
                }
            }
        }
    }
    Ok(parts.join("/"))
}

/// `path`（= `root` + `rel`）が実在し、実ファイルの `root` からの相対パスが `rel` と大文字小文字
/// だけ異なる場合に、その実際の相対パス（`/` 区切り）を返す。Windows / macOS の通常のファイル
/// システムは大文字小文字を区別せず開けるが、pak のキー照合は区別するため、開発（Dir）では
/// 動くのに配布 pak でだけ読めない事故の元になる。
fn case_mismatch(root: &Path, rel: &str, path: &Path) -> Option<String> {
    let actual = path.canonicalize().ok()?;
    let actual = actual.strip_prefix(root.canonicalize().ok()?).ok()?;
    let parts = |p: &Path| -> Vec<String> {
        p.components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect()
    };
    let (actual, requested) = (parts(actual), parts(Path::new(rel)));
    let same_ignoring_case = actual.len() == requested.len()
        && actual
            .iter()
            .zip(&requested)
            .all(|(a, r)| a.to_lowercase() == r.to_lowercase());
    (same_ignoring_case && actual != requested).then(|| actual.join("/"))
}

/// `case_mismatch` を調べて警告する。パスごとに 1 回だけ（読み込み自体は従来どおり成功させる）。
fn warn_on_case_mismatch(root: &Path, rel: &str, path: &Path) {
    static CHECKED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let first = CHECKED
        .get_or_init(Default::default)
        .lock()
        .map(|mut s| s.insert(path.to_path_buf()))
        .unwrap_or(false);
    if !first {
        return;
    }
    if let Some(actual) = case_mismatch(root, rel, path) {
        log::warn!(
            "アセット '{rel}' は実ファイル名 '{actual}' と大文字小文字が違います。この環境では読めますが、pak 化した配布版では見つかりません（pak のキーは大文字小文字を区別）"
        );
    }
}

impl GameDir {
    /// `root` がディレクトリなら従来通り `Backend::Dir` として開く。ファイル（NPK1 pak
    /// 想定）なら `Backend::Pak` として開く: 鍵は `pak::resolve_engine_key()`
    /// （`KEY_SLOT` パッチ → `NOIRIA_PAK_KEY` 環境変数 → 開発用固定鍵の優先順）で解決し、
    /// 誤った鍵は `Pak::open_file` がここで明確なエラーとして返す。
    pub fn open(root: impl Into<PathBuf>) -> Result<(Self, GameConfig)> {
        let root: PathBuf = root.into();
        let root = root
            .canonicalize()
            .with_context(|| format!("ゲームディレクトリが見つかりません: {}", root.display()))?;
        if root.is_file() {
            let key = crate::pak::resolve_engine_key().context("pak の鍵解決に失敗しました")?;
            let pak = Pak::open_file(&root, &key)
                .with_context(|| format!("pak を開けません: {}", root.display()))?;
            let text = pak
                .read("game.json")
                .context("pak 内に game.json がありません")?;
            let text = String::from_utf8(text).context("game.json が UTF-8 ではありません")?;
            let config: GameConfig =
                serde_json::from_str(&text).context("game.json のパースに失敗")?;
            return Ok((
                Self {
                    backend: Arc::new(Backend::Pak {
                        pak,
                        pseudo_root: root,
                    }),
                },
                config,
            ));
        }
        let config_path = root.join("game.json");
        let text = fs::read_to_string(&config_path)
            .with_context(|| format!("game.json が読めません: {}", config_path.display()))?;
        let config: GameConfig = serde_json::from_str(&text).context("game.json のパースに失敗")?;
        Ok((
            Self {
                backend: Arc::new(Backend::Dir(root)),
            },
            config,
        ))
    }

    /// ゲームルートを表す実パス相当の値。V8/tsint モジュールローダが `file://` の
    /// 基点 URL を組み立てるためだけに使う（アセット読込自体は下の各メソッド経由に
    /// 統一されている）。`Backend::Pak` では実ディレクトリではなく pak ファイル自身の
    /// 実パスを返す「疑似ルート」（`file://` URL の組み立てに使うだけで、実際にこの
    /// パスへファイルアクセスすることはない。全読込は `read_text` 等が pak 内部から
    /// 行う。`docs/implementation-notes.md` 2026-07-12 W6 参照）。
    pub fn root(&self) -> &Path {
        match self.backend.as_ref() {
            Backend::Dir(root) => root,
            Backend::Pak { pseudo_root, .. } => pseudo_root,
        }
    }

    /// ゲームルート相対パスを解決する。ルート外への脱出 (.. / 絶対パス) は拒否。
    /// `Backend::Dir` 専用の内部処理なので非公開（W1〜W5 まで公開だったが、
    /// 呼び出し側が絶対パスを持ち回るのを避けるため W5 で非公開化した。
    /// 実ファイルパスが必要なのは `open_video` の Dir 分岐だけ）。
    fn resolve(&self, rel: &str) -> Result<PathBuf> {
        check_jail(rel)?;
        let Backend::Dir(root) = self.backend.as_ref() else {
            bail!("resolve() は Backend::Dir 専用です（内部バグ）: {rel}");
        };
        let path = root.join(Path::new(rel));
        warn_on_case_mismatch(root, rel, &path);
        Ok(path)
    }

    pub fn read_text(&self, rel: &str) -> Result<String> {
        match self.backend.as_ref() {
            Backend::Dir(_) => {
                let path = self.resolve(rel)?;
                fs::read_to_string(&path)
                    .with_context(|| format!("ファイルが読めません: {}", path.display()))
            }
            Backend::Pak { pak, .. } => {
                let key = pak_key(rel)?;
                let bytes = pak
                    .read(&key)
                    .with_context(|| format!("ファイルが読めません: {rel}"))?;
                String::from_utf8(bytes).with_context(|| format!("ファイルが読めません: {rel}"))
            }
        }
    }

    pub fn exists(&self, rel: &str) -> bool {
        match self.backend.as_ref() {
            Backend::Dir(_) => self.resolve(rel).map(|p| p.exists()).unwrap_or(false),
            Backend::Pak { pak, .. } => pak_key(rel).map(|k| pak.exists(&k)).unwrap_or(false),
        }
    }

    /// バイナリファイルを読む（フォント等、UTF-8 テキストでないファイル用）。
    pub fn read_bytes(&self, rel: &str) -> Result<Vec<u8>> {
        match self.backend.as_ref() {
            Backend::Dir(_) => {
                let path = self.resolve(rel)?;
                fs::read(&path).with_context(|| format!("ファイルが読めません: {}", path.display()))
            }
            Backend::Pak { pak, .. } => {
                let key = pak_key(rel)?;
                pak.read(&key)
                    .with_context(|| format!("ファイルが読めません: {rel}"))
            }
        }
    }

    /// バイナリファイルをシーク可能なストリームとして開く（ストリーミング音声デコード用）。
    pub fn open_reader(&self, rel: &str) -> Result<AssetReader> {
        match self.backend.as_ref() {
            Backend::Dir(_) => {
                let path = self.resolve(rel)?;
                let file = fs::File::open(&path)
                    .with_context(|| format!("ファイルが読めません: {}", path.display()))?;
                let len = file
                    .metadata()
                    .with_context(|| format!("ファイルが読めません: {}", path.display()))?
                    .len();
                Ok(AssetReader::File { file, len })
            }
            Backend::Pak { pak, .. } => {
                let key = pak_key(rel)?;
                if pak.codec_of(&key) == Some(CODEC_STORED) {
                    let reader = pak
                        .open_entry_reader(&key)
                        .with_context(|| format!("ファイルが読めません: {rel}"))?;
                    return Ok(AssetReader::Pak(reader));
                }
                let bytes = pak
                    .read(&key)
                    .with_context(|| format!("ファイルが読めません: {rel}"))?;
                Ok(AssetReader::Memory(Cursor::new(bytes)))
            }
        }
    }

    /// `rel` 直下のファイル名一覧（1 階層、ディレクトリ自体は含まない）。
    /// ディレクトリ（`Backend::Pak` では該当プレフィクスのエントリ）が存在しない場合は
    /// エラーにせず空 Vec を返す（`fonts/` 等、同梱が任意のディレクトリを列挙する用途の
    /// ため。順序はファイル名昇順で安定させる）。
    pub fn list_dir(&self, rel: &str) -> Result<Vec<String>> {
        match self.backend.as_ref() {
            Backend::Dir(_) => {
                let path = self.resolve(rel)?;
                let Ok(read_dir) = fs::read_dir(&path) else {
                    return Ok(Vec::new());
                };
                let mut names: Vec<String> = read_dir
                    .flatten()
                    .filter(|entry| entry.path().is_file())
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .collect();
                names.sort();
                Ok(names)
            }
            Backend::Pak { pak, .. } => {
                let key = pak_key(rel)?;
                Ok(pak.list_dir(&key))
            }
        }
    }

    /// 画像を読み込みプリマルチプライ済み Pixmap にする。
    pub fn load_image(&self, rel: &str) -> Result<Pixmap> {
        let (w, h, rgba) = self.load_image_rgba(rel)?;
        Ok(Pixmap::from_rgba_straight(w, h, &rgba))
    }

    /// straight RGBA で読み込む。PNG (8bit 非インターレース)・JPEG (ベースライン)・
    /// WebP は対応、それ以外は明確なエラーを返す。デコードロジック自体
    /// (`decode_image_bytes`) は `Backend::Dir`/`Backend::Pak` で共通（W6 で切り出した）。
    pub fn load_image_rgba(&self, rel: &str) -> Result<(u32, u32, Vec<u8>)> {
        match self.backend.as_ref() {
            Backend::Dir(_) => {
                let path = self.resolve(rel)?;
                let bytes = fs::read(&path)
                    .with_context(|| format!("画像が読めません: {}", path.display()))?;
                decode_image_bytes(&bytes, &path.display().to_string())
            }
            Backend::Pak { pak, .. } => {
                let key = pak_key(rel)?;
                let bytes = pak
                    .read(&key)
                    .with_context(|| format!("画像が読めません: {rel}"))?;
                decode_image_bytes(&bytes, rel)
            }
        }
    }

    /// 動画を開く（`rel` はゲームルート相対）。`Backend::Dir` は従来どおり実パスで
    /// `codec::video::open_video_file` に委譲する（Windows の Media Foundation
    /// フォールバックは実ファイルパス前提のため、この分岐にのみ実パスが残る）。
    /// `Backend::Pak` は pak から読んだバイト列を `codec::video::open_video`
    /// （`Vec<u8>` 全所有版、AVI/IVF 対応）に渡す。MP4 等 Media Foundation でしか
    /// 再生できない形式は実ファイルパスが無いと開けないため、pak 内からは明確な
    /// エラーにする（一時ファイルへの書き出しは行わない方針。
    /// `docs/implementation-notes.md` 2026-07-12 W6 参照）。
    pub fn open_video(&self, rel: &str) -> Result<Box<dyn VideoSource>> {
        match self.backend.as_ref() {
            Backend::Dir(_) => {
                let path = self.resolve(rel)?;
                crate::codec::video::open_video_file(&path)
            }
            Backend::Pak { pak, .. } => {
                let key = pak_key(rel)?;
                let bytes = pak
                    .read(&key)
                    .with_context(|| format!("動画が読めません: {rel}"))?;
                if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
                    bail!(
                        "pak 内から MP4 は再生できません（Media Foundation は実ファイルパス前提のため）。\
                         AVI または IVF を使用してください: {rel}"
                    );
                }
                crate::codec::video::open_video(bytes)
            }
        }
    }

    /// テスト専用: `game.json` を要求せずディレクトリを直接開く（`Backend::Dir`）。
    /// `noiria-core` 内のユニットテストが `GameDir` を必要とするが、
    /// 実ゲームディレクトリ（`game.json` 同梱）を用意するまでもない箇所で使う。
    #[cfg(test)]
    pub(crate) fn for_test(root: impl Into<PathBuf>) -> Self {
        Self {
            backend: Arc::new(Backend::Dir(root.into())),
        }
    }
}

/// PNG/JPEG/WebP のマジックバイトを見て対応デコーダへ振り分ける（`load_image_rgba` の
/// `Backend::Dir`/`Backend::Pak` 共通処理）。拡張子ではなくマジックバイトで判定する
/// ため、pack が `.png` パスのまま中身だけ WebP に差し替えても正しく振り分けられる
/// （`docs/implementation-notes.md` 2026-07-12 配布対応 W8 参照）。`label` はエラー
/// メッセージ表示用（`Backend::Dir` は実パス、`Backend::Pak` は pak 内相対パスを渡す）。
fn decode_image_bytes(bytes: &[u8], label: &str) -> Result<(u32, u32, Vec<u8>)> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        let png = crate::codec::png::decode(bytes).with_context(|| {
            format!("PNG は 8bit 非インターレースのみ対応です。画像を変換してください: {label}")
        })?;
        return Ok((png.width, png.height, png.rgba));
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let jpg = crate::codec::jpeg::decode(bytes)
            .with_context(|| format!("JPEG のデコードに失敗しました: {label}"))?;
        return Ok((jpg.width, jpg.height, jpg.rgba));
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let (width, height, rgba) = crate::codec::webp::decode_rgba(bytes)
            .with_context(|| format!("WebP のデコードに失敗しました: {label}"))?;
        return Ok((width, height, rgba));
    }
    bail!("非対応の画像形式です (PNG・JPEG・WebP のみ対応): {label}");
}

/// Pixmap を PNG として保存する（スクリーンショット/サムネイル用）。
pub fn save_png(pixmap: &Pixmap, path: &Path) -> Result<()> {
    // premultiplied → straight へ戻す
    let mut straight = Vec::with_capacity(pixmap.data.len());
    for px in pixmap.data.as_chunks::<4>().0 {
        let a = px[3] as u32;
        // a == 0 は None -> 0 で RGBA 全て 0 になる（従来の a == 0 分岐と同じ）。
        let unpremultiply = |c: u8| (c as u32 * 255).checked_div(a).unwrap_or(0).min(255) as u8;
        straight.extend_from_slice(&[
            unpremultiply(px[0]),
            unpremultiply(px[1]),
            unpremultiply(px[2]),
            px[3],
        ]);
    }
    let png = crate::codec::png::encode(pixmap.width, pixmap.height, &straight);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, png).with_context(|| format!("PNG 保存に失敗: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_jail() {
        let dir = GameDir::for_test(std::env::temp_dir());
        assert!(dir.resolve("assets/bg/room.png").is_ok());
        assert!(dir.resolve("../secret.txt").is_err());
        // ドライブレター付き絶対パスは Windows でのみ意味を持つ脱出経路（`Path::is_absolute`
        // が真になるのは Windows だけ）。Linux では `\` はただの文字なので単一の
        // Normal コンポーネント扱いになり、ルート配下の一風変わったファイル名として
        // 正当に解決される（W1: WSL での初回検証で判明。既存の脱出防止ロジック自体は
        // 変更していない — `..`/`/` によるジェイルブレイクは両 OS で引き続き拒否される）。
        #[cfg(target_os = "windows")]
        assert!(dir.resolve("C:\\Windows\\system.ini").is_err());
        assert!(dir.resolve("/etc/passwd").is_err());
    }

    /// W5: 公開 API (`read_bytes`/`read_text`/`open_video`) 経由でも `../` 脱出が
    /// 拒否されることを確認する（`resolve` 非公開化後も、これらのメソッド内部で
    /// 同じジェイルロジックが効いていることの回帰テスト）。
    #[test]
    fn path_jail_via_public_read_apis() {
        let dir = GameDir::for_test(std::env::temp_dir());
        assert!(dir.read_bytes("../secret.bin").is_err());
        assert!(dir.read_text("../secret.txt").is_err());
        assert!(dir.open_video("../secret.avi").is_err());
        assert!(!dir.exists("../secret.bin"));
    }

    #[test]
    fn list_dir_lists_files_sorted_and_ignores_subdirs() {
        let root = std::env::temp_dir().join(format!(
            "noiria_list_dir_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let fonts = root.join("fonts");
        fs::create_dir_all(fonts.join("nested")).unwrap();
        fs::write(fonts.join("b.ttf"), b"b").unwrap();
        fs::write(fonts.join("a.ttc"), b"a").unwrap();
        fs::write(fonts.join("nested").join("c.ttf"), b"c").unwrap();

        let dir = GameDir::for_test(root.clone());
        let names = dir.list_dir("fonts").expect("list_dir");
        assert_eq!(
            names,
            vec!["a.ttc".to_string(), "b.ttf".to_string()],
            "1階層のみ・昇順"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn list_dir_missing_directory_returns_empty_not_error() {
        let dir = GameDir::for_test(std::env::temp_dir());
        let names = dir
            .list_dir("this_dir_definitely_does_not_exist_xyz")
            .expect("list_dir");
        assert!(names.is_empty());
    }

    /// W8: `decode_image_bytes` が拡張子ではなくマジックバイトで WebP を判定できる
    /// ことの回帰テスト（pack が `.png` パスのまま中身を WebP に差し替える前提のため）。
    #[test]
    fn decode_image_bytes_sniffs_webp_by_magic_regardless_of_label() {
        let rgba = vec![1u8, 2, 3, 255, 4, 5, 6, 128, 7, 8, 9, 0, 10, 11, 12, 255];
        let webp = crate::codec::webp::encode_lossless(2, 2, &rgba).unwrap();
        let (w, h, decoded) = decode_image_bytes(&webp, "bg/room.png").unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(decoded, rgba);
    }

    #[test]
    fn read_bytes_reads_binary_content() {
        let root = std::env::temp_dir().join(format!(
            "noiria_read_bytes_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("blob.bin"), [0u8, 1, 2, 255]).unwrap();
        let dir = GameDir::for_test(root.clone());
        assert_eq!(dir.read_bytes("blob.bin").unwrap(), vec![0u8, 1, 2, 255]);
        let _ = fs::remove_dir_all(&root);
    }
    // --- R1: Dir バックエンドの大文字小文字（pak は区別するので、開発側で気付けるようにする） ---

    #[test]
    fn case_mismatch_is_reported_but_the_read_still_succeeds() {
        let root = std::env::temp_dir().join(format!(
            "noiria_case_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("BG")).unwrap();
        fs::write(root.join("BG").join("Room.PNG"), b"x").unwrap();
        // 大文字小文字を区別するファイルシステム（Linux 等）では、そもそも別名では開けない。
        let insensitive = root.join("bg").join("room.png").exists();

        let dir = GameDir::for_test(root.clone());
        let exact = root.join("BG").join("Room.PNG");
        assert_eq!(case_mismatch(&root, "BG/Room.PNG", &exact), None);
        if insensitive {
            let wrong = root.join("bg").join("room.png");
            assert_eq!(
                case_mismatch(&root, "bg/room.png", &wrong).as_deref(),
                Some("BG/Room.PNG")
            );
            assert_eq!(dir.read_bytes("bg/room.png").unwrap(), b"x", "読み込みは成功させる");
        }
        // 実在しないパスは警告対象にしない。
        assert_eq!(
            case_mismatch(&root, "nope/none.png", &root.join("nope").join("none.png")),
            None
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn open_pak_file_reads_on_demand_and_enforces_jail() {
        let root = std::env::temp_dir().join(format!(
            "noiria_gamedir_pak_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let pak_path = root.join("game.pak");

        let game_json = br#"{"title":"PakTest","id":"pak-test"}"#.to_vec();
        let key = crate::pak::dev_key();
        let (pak_bytes, _) = crate::pak::build(
            vec![
                ("game.json".to_string(), game_json.clone()),
                ("scenario/first.ks".to_string(), b"hello pak".to_vec()),
                ("fonts/a.ttf".to_string(), vec![1, 2, 3, 4]),
            ],
            &key,
        );
        fs::write(&pak_path, &pak_bytes).unwrap();

        {
            let (dir, config) = GameDir::open(&pak_path).unwrap();
            assert_eq!(config.title, "PakTest");
            assert_eq!(config.id, "pak-test");
            assert!(dir.exists("scenario/first.ks"));
            assert_eq!(dir.read_text("scenario/first.ks").unwrap(), "hello pak");
            assert_eq!(dir.read_bytes("fonts/a.ttf").unwrap(), vec![1, 2, 3, 4]);
            assert_eq!(dir.list_dir("fonts").unwrap(), vec!["a.ttf".to_string()]);
            assert!(dir.read_bytes("../secret.bin").is_err());
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn open_reader_supports_dir_pak_stored_and_pak_compressed_and_enforces_jail() {
        let root = std::env::temp_dir().join(format!(
            "noiria_open_reader_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();

        // Backend::Dir
        let payload = b"0123456789abcdef".to_vec();
        fs::write(root.join("clip.wav"), &payload).unwrap();
        let dir = GameDir::for_test(root.clone());
        let mut r = dir.open_reader("clip.wav").unwrap();
        assert_eq!(r.len(), payload.len() as u64);
        assert!(matches!(r, AssetReader::File { .. }));
        r.seek(SeekFrom::Start(6)).unwrap();
        let mut slice = [0u8; 4];
        r.read_exact(&mut slice).unwrap();
        assert_eq!(&slice, b"6789");
        assert!(dir.open_reader("../secret.wav").is_err());

        // Backend::Pak (stored OGG + deflated compressible data)
        let pak_path = root.join("game.pak");
        let game_json = br#"{"title":"StreamTest","id":"stream-test"}"#.to_vec();
        let compressible = vec![b'A'; 4096];
        let key = crate::pak::dev_key();
        let (pak_bytes, _) = crate::pak::build(
            vec![
                ("game.json".to_string(), game_json),
                ("bgm/theme.ogg".to_string(), payload.clone()),
                ("bgm/tone.wav".to_string(), compressible.clone()),
            ],
            &key,
        );
        fs::write(&pak_path, &pak_bytes).unwrap();
        {
            let (pak_dir, _) = GameDir::open(&pak_path).unwrap();
            let mut stored_r = pak_dir.open_reader("bgm/theme.ogg").unwrap();
            assert!(matches!(stored_r, AssetReader::Pak(_)));
            assert_eq!(stored_r.len(), payload.len() as u64);
            stored_r.seek(SeekFrom::Start(10)).unwrap();
            let mut out = Vec::new();
            stored_r.read_to_end(&mut out).unwrap();
            assert_eq!(out, &payload[10..]);

            let mut deflated_r = pak_dir.open_reader("bgm/tone.wav").unwrap();
            assert!(matches!(deflated_r, AssetReader::Memory(_)));
            assert_eq!(deflated_r.len(), compressible.len() as u64);
            let mut out = Vec::new();
            deflated_r.read_to_end(&mut out).unwrap();
            assert_eq!(out, compressible);

            assert!(pak_dir.open_reader("../secret.ogg").is_err());
            assert!(pak_dir.open_reader("bgm/missing.ogg").is_err());
        }

        let _ = fs::remove_dir_all(&root);
    }
}
