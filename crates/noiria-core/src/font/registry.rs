//! フォントレジストリ (F3): OS ごとのシステムフォントディレクトリを再帰走査し、
//! family 名 → (フォントの実体参照, TTC インデックス) の索引をプロセス起動後
//! 1 回だけ構築する。`text.rs` の `TextEngine` が family 名からフォントを
//! 解決するための土台（ゲームローカルフォントは `TextEngine` 側が別インスタンスの
//! `FontRegistry` として保持し、こちらの静的システムレジストリより優先する）。
//!
//! - 対応拡張子は `.ttf` / `.ttc` のみ（仕様どおり）。CFF/'OTTO' フォントは
//!   `Font::from_bytes` が `Err` を返すため自然にスキップされる。
//! - TTC は含まれる全フォントを個別に索引する（`ttcf` ヘッダの `numFonts` を
//!   先読みしてインデックスを 0..numFonts で列挙する）。
//! - 1 ファイルの解析に失敗しても全体のスキャンは継続する（該当ファイルだけスキップ）。
//! - family 名は `name` テーブルの nameID=1 の「最良スコアの 1 件」（`Font::family_name`）
//!   だけでなく、全プラットフォーム/言語のレコードを索引に加える。これにより
//!   日本語ローカライズ名（例: "游ゴシック"）でも解決できる。
//! - ディレクトリ走査は再帰的（深さ上限 [`MAX_SCAN_DEPTH`]）。シンボリックリンクの
//!   循環はこの深さ上限だけで対策する（W1 設計判断: 追加のループ検出は行わない）。

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use super::tables;
use super::{sfnt, Font};

/// 再帰走査の深さ上限（シンボリックリンクのループ対策を兼ねる）。
const MAX_SCAN_DEPTH: u32 = 6;

/// システムフォントディレクトリの一覧（存在しないディレクトリは走査時に自然にスキップされる）。
#[cfg(target_os = "windows")]
fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from(r"C:\Windows\Fonts")];
    // ユーザー単位インストールされたフォント（管理者権限不要でインストールされる分）。
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local_app_data).join(r"Microsoft\Windows\Fonts"));
    }
    dirs
}

#[cfg(not(target_os = "windows"))]
fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
    ];
    // fontconfig には依存しない（`directories` クレートの BaseDirs でホームだけ解決する）。
    if let Some(base) = directories::BaseDirs::new() {
        dirs.push(base.home_dir().join(".local/share/fonts"));
        dirs.push(base.home_dir().join(".fonts"));
    }
    dirs
}

/// レジストリに登録された 1 フォントへの実体参照。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FontSource {
    /// ディスク上のファイルパス（システムフォント・W1 以前の唯一の形態）。
    Path(PathBuf),
    /// メモリ上に既に読み込まれたバイト列（ゲームローカルフォント用。W6 の暗号化 pak
    /// からの読込でもファイルパスを経由せずそのまま注入できるようにするための形態）。
    /// `Eq`/`Hash` は内容比較ではなく `Arc` の同一性で行う（レジストリが唯一の保有元で、
    /// `FontKey` はそこから複製されたものだけが流通する前提のため、同一性比較で十分）。
    Bytes(ArcBytes),
}

/// [`FontSource::Bytes`] 用の同一性比較付きラッパ。
#[derive(Clone, Debug)]
pub struct ArcBytes(pub Arc<[u8]>);

impl PartialEq for ArcBytes {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for ArcBytes {}
impl std::hash::Hash for ArcBytes {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as *const u8 as usize).hash(state);
    }
}

/// レジストリに登録された 1 フォントへの参照（実体参照 + TTC 内インデックス）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontKey {
    pub source: FontSource,
    pub index: u32,
}

struct Entry {
    key: FontKey,
}

/// システムフォントの索引。`global()` でプロセス内 1 個の共有インスタンスを得る。
pub struct FontRegistry {
    /// family 名（ASCII 大文字小文字無視で正規化）→ 最初に見つかったフォント。
    by_family: HashMap<String, FontKey>,
    /// グリフ単位フォールバック走査用の全登録フォント一覧。
    all: Vec<Entry>,
}

impl FontRegistry {
    /// 空のレジストリ（`TextEngine` がゲームローカルフォント未同梱時の既定値として使う）。
    pub(crate) fn empty() -> Self {
        Self {
            by_family: HashMap::new(),
            all: Vec::new(),
        }
    }

    /// OS ごとのシステムフォントディレクトリ（[`system_font_dirs`]）を再帰走査して
    /// レジストリを構築する。1 件も見つからない場合は、走査した全ディレクトリを
    /// 添えて warn ログを出す（「黙って文字が消える」状態を避けるため）。
    pub fn scan_system_fonts() -> Self {
        Self::scan_dirs(&system_font_dirs())
    }

    /// [`scan_system_fonts`] の本体（テストから任意のディレクトリ集合を渡せるように分離）。
    fn scan_dirs(dirs: &[PathBuf]) -> Self {
        let mut reg = Self::empty();
        for dir in dirs {
            reg.scan_dir_recursive(dir, 0);
        }
        if reg.is_empty() {
            let listed = dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            log::warn!(
                "システムフォントが1件も見つかりませんでした（走査したディレクトリ: {listed}）。\
                 文字が描画されません。フォントをインストールするか、ゲームの fonts/ に \
                 .ttf/.ttc を同梱してください。"
            );
        }
        reg
    }

    /// `dir` を再帰的に走査する。`depth` が [`MAX_SCAN_DEPTH`] を超えたら打ち切る
    /// （シンボリックリンクの循環対策）。ディレクトリが存在しない/読めない場合は
    /// 何もしない（呼び出し元の複数ディレクトリ走査を継続させるため panic しない）。
    fn scan_dir_recursive(&mut self, dir: &Path, depth: u32) {
        if depth > MAX_SCAN_DEPTH {
            return;
        }
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                self.scan_dir_recursive(&path, depth + 1);
                continue;
            }
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            let ext_lower = ext.to_ascii_lowercase();
            if ext_lower != "ttf" && ext_lower != "ttc" {
                continue;
            }
            self.index_file(&path);
        }
    }

    /// 1 ファイルを最小限のシーク読込（ヘッダ・テーブルディレクトリ・小さな検証用テーブル・
    /// `name` テーブルのみ）で索引する。全ファイル読込＋`glyf` 読み込みを避けて起動時間を短縮する。
    fn index_file(&mut self, path: &Path) {
        let Ok(mut file) = File::open(path) else {
            return;
        };
        let Ok(file_len) = file.metadata().map(|m| m.len()) else {
            return;
        };
        if file_len < 12 {
            return;
        }
        let mut hdr = [0u8; 12];
        if file.read_exact(&mut hdr).is_err() {
            return;
        }
        let sfnt_offsets: Vec<u64> = if &hdr[0..4] == b"ttcf" {
            let declared = u32::from_be_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]);
            let fits = ((file_len - 12) / 4).min(u32::MAX as u64) as u32;
            let count = declared.min(fits).min(1024);
            if count == 0 {
                return;
            }
            let mut raw = vec![0u8; count as usize * 4];
            if file.read_exact(&mut raw).is_err() {
                return;
            }
            raw.as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_be_bytes(*c) as u64)
                .collect()
        } else {
            vec![0]
        };

        let mut dir_buf = Vec::new();
        let mut name_buf = Vec::new();
        let mut cmap_hdr = Vec::new();
        for (index, sfnt_offset) in sfnt_offsets.into_iter().enumerate() {
            let Some(sfnt_end) = sfnt_offset.checked_add(12) else {
                continue;
            };
            if sfnt_end > file_len || file.seek(SeekFrom::Start(sfnt_offset)).is_err() {
                continue;
            }
            let mut sh = [0u8; 12];
            if file.read_exact(&mut sh).is_err() {
                continue;
            }
            let version = u32::from_be_bytes([sh[0], sh[1], sh[2], sh[3]]);
            if version != 0x0001_0000 && version != 0x7472_7565 {
                continue;
            }
            let num_tables = u16::from_be_bytes([sh[4], sh[5]]) as usize;
            let records_len = num_tables * 16;
            if sfnt_end.saturating_add(records_len as u64) > file_len {
                continue;
            }
            dir_buf.resize(records_len, 0);
            if file.read_exact(&mut dir_buf).is_err() {
                continue;
            }

            let mut head_loc = None;
            let mut maxp_loc = None;
            let mut hhea_loc = None;
            let mut hmtx_loc = None;
            let mut cmap_loc = None;
            let mut loca_loc = None;
            let mut glyf_loc = None;
            let mut name_loc = None;
            for rec in dir_buf.as_chunks::<16>().0 {
                let tag: [u8; 4] = [rec[0], rec[1], rec[2], rec[3]];
                let off = u32::from_be_bytes([rec[8], rec[9], rec[10], rec[11]]) as u64;
                let len = u32::from_be_bytes([rec[12], rec[13], rec[14], rec[15]]) as u64;
                let slot = match &tag {
                    b"head" => &mut head_loc,
                    b"maxp" => &mut maxp_loc,
                    b"hhea" => &mut hhea_loc,
                    b"hmtx" => &mut hmtx_loc,
                    b"cmap" => &mut cmap_loc,
                    b"loca" => &mut loca_loc,
                    b"glyf" => &mut glyf_loc,
                    b"name" => &mut name_loc,
                    _ => continue,
                };
                *slot = Some((off, len));
            }

            let (
                Some((head_off, head_len)),
                Some((maxp_off, maxp_len)),
                Some((hhea_off, hhea_len)),
                Some((hmtx_off, hmtx_len)),
                Some((cmap_off, cmap_len)),
                Some((loca_off, loca_len)),
                Some((glyf_off, glyf_len)),
            ) = (
                head_loc, maxp_loc, hhea_loc, hmtx_loc, cmap_loc, loca_loc, glyf_loc,
            )
            else {
                continue;
            };

            let fits_file = |off: u64, len: u64| off.checked_add(len).is_some_and(|e| e <= file_len);
            if !fits_file(head_off, head_len)
                || !fits_file(maxp_off, maxp_len)
                || !fits_file(hhea_off, hhea_len)
                || !fits_file(hmtx_off, hmtx_len)
                || !fits_file(cmap_off, cmap_len)
                || !fits_file(loca_off, loca_len)
                || !fits_file(glyf_off, glyf_len)
            {
                continue;
            }
            if head_len < 54 || maxp_len < 6 || hhea_len < 36 || cmap_len < 4 {
                continue;
            }

            let mut head_bytes = [0u8; 54];
            if read_exact_at(&mut file, head_off, &mut head_bytes).is_err() {
                continue;
            }
            let Ok(head) = tables::parse_head(&head_bytes) else {
                continue;
            };
            let mut maxp_bytes = [0u8; 6];
            if read_exact_at(&mut file, maxp_off, &mut maxp_bytes).is_err() {
                continue;
            }
            let Ok(num_glyphs) = tables::parse_maxp_num_glyphs(&maxp_bytes) else {
                continue;
            };
            let mut hhea_bytes = [0u8; 36];
            if read_exact_at(&mut file, hhea_off, &mut hhea_bytes).is_err() {
                continue;
            }
            let Ok(hhea) = tables::parse_hhea(&hhea_bytes) else {
                continue;
            };
            if hmtx_len < (hhea.number_of_h_metrics as u64) * 4 {
                continue;
            }
            let loca_entry_bytes = if head.index_to_loc_format == 1 { 4u64 } else { 2u64 };
            if loca_len < (num_glyphs as u64 + 1) * loca_entry_bytes {
                continue;
            }
            if !file_has_supported_cmap(&mut file, cmap_off, cmap_len, &mut cmap_hdr) {
                continue;
            }

            let key = FontKey {
                source: FontSource::Path(path.to_path_buf()),
                index: index as u32,
            };
            if let Some((name_off, name_len)) = name_loc {
                if fits_file(name_off, name_len) && name_len <= 16 << 20 {
                    name_buf.resize(name_len as usize, 0);
                    if read_exact_at(&mut file, name_off, &mut name_buf).is_ok() {
                        self.register_name_table(&name_buf, &key);
                    }
                }
            }
            self.all.push(Entry { key });
        }
    }

    fn register_name_table(&mut self, name_bytes: &[u8], key: &FontKey) {
        if let Some(primary) = tables::parse_family_name(name_bytes) {
            if !primary.is_empty() {
                self.register(&primary, key.clone());
            }
        }
        for (_, _, alt_family) in tables::parse_family_names(name_bytes) {
            if !alt_family.is_empty() {
                self.register(&alt_family, key.clone());
            }
        }
    }

    /// ゲームローカルフォント（`fonts/*.ttf|ttc`、W1）をメモリ上のバイト列から索引する。
    /// `files` の順序がそのまま登録順（`iter_all`/`any` で先頭に来る = フォールバック優先度）
    /// になるため、呼び出し側（`engine_state.rs`）でファイル名昇順に揃えて渡すこと。
    pub(crate) fn from_game_font_bytes(files: &[Vec<u8>]) -> Self {
        let mut reg = Self::empty();
        for bytes in files {
            let shared: Arc<[u8]> = Arc::from(bytes.as_slice());
            let before = reg.len();
            reg.index_bytes(bytes, |index| FontKey {
                source: FontSource::Bytes(ArcBytes(shared.clone())),
                index,
            });
            if reg.len() == before {
                let reason = Font::from_bytes(bytes.clone(), 0)
                    .err()
                    .map_or_else(|| "不明な理由".to_string(), |e| format!("{e:#}"));
                log::error!("ゲームフォントを読み込めませんでした（無視します）: {reason}");
            }
        }
        reg
    }

    /// 1 ファイル/1 バイト列（TTC なら複数フォント）を索引する共通ロジック。
    /// `key_for` が TTC インデックスから実際に登録する [`FontKey`] を作る
    /// （ファイル参照かメモリ参照かの違いだけを外側で吸収する）。
    fn index_bytes(&mut self, bytes: &[u8], key_for: impl Fn(u32) -> FontKey) {
        let count = probe_font_count(bytes);
        for index in 0..count {
            let Ok(dir) = sfnt::parse_table_directory(bytes, index) else {
                continue;
            };
            let Ok(head_bytes) = dir.require(bytes, b"head") else {
                continue;
            };
            let Ok(head) = tables::parse_head(head_bytes) else {
                continue;
            };
            let Ok(maxp_bytes) = dir.require(bytes, b"maxp") else {
                continue;
            };
            let Ok(num_glyphs) = tables::parse_maxp_num_glyphs(maxp_bytes) else {
                continue;
            };
            let Ok(hhea_bytes) = dir.require(bytes, b"hhea") else {
                continue;
            };
            let Ok(hhea) = tables::parse_hhea(hhea_bytes) else {
                continue;
            };
            let Ok(hmtx_bytes) = dir.require(bytes, b"hmtx") else {
                continue;
            };
            if hmtx_bytes.len() < (hhea.number_of_h_metrics as usize) * 4 {
                continue;
            }
            let Ok(cmap_bytes) = dir.require(bytes, b"cmap") else {
                continue;
            };
            if !slice_has_supported_cmap(cmap_bytes) {
                continue;
            }
            let Ok(loca_bytes) = dir.require(bytes, b"loca") else {
                continue;
            };
            let loca_entry_bytes = if head.index_to_loc_format == 1 { 4 } else { 2 };
            if loca_bytes.len() < (num_glyphs as usize + 1) * loca_entry_bytes {
                continue;
            }
            if dir.require(bytes, b"glyf").is_err() {
                continue;
            }

            let key = key_for(index);
            if let Ok(Some(name_bytes)) = dir.find_slice(bytes, b"name") {
                self.register_name_table(name_bytes, &key);
            }
            self.all.push(Entry { key });
        }
    }

    fn register(&mut self, family: &str, key: FontKey) {
        // 最初に見つかったフォントを優先する（同名衝突時は先勝ち。決定的な順序は
        // OS のディレクトリ列挙順に依存するが、フォールバック用途では十分）。
        self.by_family.entry(normalize(family)).or_insert(key);
    }

    /// 完全一致（ASCII 大文字小文字無視）で family を解決する。
    pub fn resolve(&self, family: &str) -> Option<FontKey> {
        self.by_family.get(&normalize(family)).cloned()
    }

    /// レジストリ内の任意の 1 フォント（何も見つからない場合の最終フォールバック用）。
    pub fn any(&self) -> Option<FontKey> {
        self.all.first().map(|e| e.key.clone())
    }

    /// 登録済み全フォント（グリフ単位フォールバックの走査用）。
    pub fn iter_all(&self) -> impl Iterator<Item = &FontKey> {
        self.all.iter().map(|e| &e.key)
    }

    pub fn len(&self) -> usize {
        self.all.len()
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }
}

fn read_exact_at(file: &mut File, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf)
}

fn file_has_supported_cmap(file: &mut File, cmap_off: u64, cmap_len: u64, buf: &mut Vec<u8>) -> bool {
    let mut hdr = [0u8; 4];
    if read_exact_at(file, cmap_off, &mut hdr).is_err() {
        return false;
    }
    let num_tables = u16::from_be_bytes([hdr[2], hdr[3]]) as u64;
    let recs_len = num_tables * 8;
    if 4 + recs_len > cmap_len {
        return false;
    }
    buf.resize(recs_len as usize, 0);
    if file.read_exact(buf).is_err() {
        return false;
    }
    for rec in buf.as_chunks::<8>().0 {
        let sub_off = u32::from_be_bytes([rec[4], rec[5], rec[6], rec[7]]) as u64;
        if sub_off + 2 > cmap_len {
            continue;
        }
        let mut fmt_bytes = [0u8; 2];
        if read_exact_at(file, cmap_off + sub_off, &mut fmt_bytes).is_err() {
            continue;
        }
        let fmt = u16::from_be_bytes(fmt_bytes);
        if fmt == 4 && sub_off + 14 <= cmap_len {
            return true;
        }
        if fmt == 12 && sub_off + 16 <= cmap_len {
            return true;
        }
    }
    false
}

fn slice_has_supported_cmap(data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }
    let num_tables = u16::from_be_bytes([data[2], data[3]]) as usize;
    if 4 + num_tables * 8 > data.len() {
        return false;
    }
    for rec in data[4..4 + num_tables * 8].as_chunks::<8>().0 {
        let sub_off = u32::from_be_bytes([rec[4], rec[5], rec[6], rec[7]]) as usize;
        if sub_off + 2 > data.len() {
            continue;
        }
        let fmt = u16::from_be_bytes([data[sub_off], data[sub_off + 1]]);
        if (fmt == 4 && sub_off + 14 <= data.len()) || (fmt == 12 && sub_off + 16 <= data.len()) {
            return true;
        }
    }
    false
}

fn normalize(family: &str) -> String {
    family.trim().to_ascii_uppercase()
}

/// ファイル先頭バイトから TTC 内のフォント数を読み取る（'ttcf' でなければ単体で 1）。
/// 壊れたヘッダでも panic しない（範囲チェック付き）。numFonts はファイル由来なので、
/// 12 バイトのヘッダの後ろに並ぶ 4 バイトのオフセット表に収まる個数までに切り詰める。
fn probe_font_count(bytes: &[u8]) -> u32 {
    if bytes.len() >= 12 && &bytes[0..4] == b"ttcf" {
        let declared = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let fits = ((bytes.len() - 12) / 4) as u32;
        declared.min(fits).max(1)
    } else {
        1
    }
}

/// プロセス内で 1 回だけスキャンする共有レジストリ（起動時コストを複数
/// `TextEngine` インスタンス間・テスト間で償却する）。
pub fn global() -> &'static FontRegistry {
    static REG: OnceLock<FontRegistry> = OnceLock::new();
    REG.get_or_init(FontRegistry::scan_system_fonts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_font_count_detects_ttc_header() {
        let mut ttc = Vec::new();
        ttc.extend_from_slice(b"ttcf");
        ttc.extend_from_slice(&1u16.to_be_bytes());
        ttc.extend_from_slice(&0u16.to_be_bytes());
        ttc.extend_from_slice(&3u32.to_be_bytes());
        ttc.extend_from_slice(&[0u8; 12]); // オフセット表 3 個ぶん
        assert_eq!(probe_font_count(&ttc), 3);

        let single = vec![0u8; 12];
        assert_eq!(probe_font_count(&single), 1);

        assert_eq!(probe_font_count(&[]), 1, "短すぎるデータでも panic しない");
    }

    #[test]
    fn probe_font_count_is_clamped_to_what_the_offset_table_can_hold() {
        // numFonts = 0xFFFFFFFF を名乗る 16 バイトの TTC（オフセット表は 1 個ぶんしかない）。
        let mut ttc = Vec::new();
        ttc.extend_from_slice(b"ttcf");
        ttc.extend_from_slice(&1u16.to_be_bytes());
        ttc.extend_from_slice(&0u16.to_be_bytes());
        ttc.extend_from_slice(&u32::MAX.to_be_bytes());
        ttc.extend_from_slice(&0u32.to_be_bytes());
        assert!(probe_font_count(&ttc) <= 1);
    }

    #[test]
    fn resolve_is_case_insensitive() {
        let mut reg = FontRegistry::empty();
        reg.register(
            "Yu Gothic UI",
            FontKey {
                source: FontSource::Path(PathBuf::from("dummy.ttc")),
                index: 0,
            },
        );
        assert!(reg.resolve("yu gothic ui").is_some());
        assert!(reg.resolve("YU GOTHIC UI").is_some());
        assert!(reg.resolve("Yu Gothic").is_none());
    }

    /// Windows 環境限定: 実システムフォント (`C:\Windows\Fonts`) でのスモークテスト。
    /// 該当フォントが無い場合はスキップする（CI/環境依存のため assert しない項目あり）。
    #[cfg(target_os = "windows")]
    #[test]
    fn scan_system_fonts_resolves_known_windows_families() {
        let reg = FontRegistry::scan_system_fonts();
        if reg.is_empty() {
            eprintln!("C:\\Windows\\Fonts が見つからない/空のためスキップ");
            return;
        }
        assert!(!reg.is_empty());
        let mut found_any_fallback_candidate = false;
        for fam in ["Yu Gothic UI", "Yu Gothic", "Meiryo", "MS Gothic"] {
            if reg.resolve(fam).is_some() {
                found_any_fallback_candidate = true;
                eprintln!("resolved: {fam}");
            }
        }
        assert!(
            found_any_fallback_candidate,
            "フォールバックチェーン候補 (Yu Gothic UI/Yu Gothic/Meiryo/MS Gothic) が \
             1 つも解決できませんでした。日本語フォントが入っていない環境の可能性があります。"
        );
    }

    // -----------------------------------------------------------------
    // W1: per-OS ディレクトリ列挙 / 再帰走査 / ゲームフォント優先 / 空レジストリ
    // -----------------------------------------------------------------

    /// 一時ディレクトリを `sub/`（さらにその下）までネストさせ、末端に TTF を置いても
    /// 再帰走査で拾われることを確認する。
    #[test]
    fn scan_dirs_recurses_into_nested_directories() {
        let base = std::env::temp_dir().join(format!(
            "noiria_font_scan_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = base.join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).expect("一時ディレクトリの作成に失敗");
        let font_bytes = super::super::tests::minimal_font_bytes("Nested Test Font");
        std::fs::write(nested.join("nested.ttf"), &font_bytes).expect("フォントの書き込みに失敗");

        let reg = FontRegistry::scan_dirs(std::slice::from_ref(&base));
        assert!(
            !reg.is_empty(),
            "ネストしたディレクトリの TTF が拾われていない"
        );
        assert!(reg.resolve("Nested Test Font").is_some());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 走査対象ディレクトリが 1 つも存在しない（≒フォント 0 件）場合でも panic せず
    /// 空のレジストリを返す（呼び出し側で明確な warn ログを出す経路）。
    #[test]
    fn scan_dirs_with_no_hits_returns_empty_without_panicking() {
        let missing = std::env::temp_dir().join("noiria_font_scan_definitely_missing_dir_xyz");
        let reg = FontRegistry::scan_dirs(&[missing]);
        assert!(reg.is_empty());
        assert!(reg.any().is_none());
    }

    /// ゲームローカルフォント（メモリ上のバイト列）が `FontSource::Bytes` として索引され、
    /// family 名で解決できることを確認する（`TextEngine.game_registry` が
    /// `registry`（システムフォント）より先に引かれることで実質的な優先になる、
    /// その前提となるレジストリ単体の挙動）。
    #[test]
    fn from_game_font_bytes_indexes_in_memory_font() {
        let bytes = super::super::tests::minimal_font_bytes("Game Local Font");
        let reg = FontRegistry::from_game_font_bytes(&[bytes]);
        assert_eq!(reg.len(), 1);
        let key = reg
            .resolve("Game Local Font")
            .expect("ゲームフォントが解決できない");
        assert!(matches!(key.source, FontSource::Bytes(_)));
        assert_eq!(reg.any(), Some(key));
    }

    /// CFF ('OTTO') の同梱フォントは索引されず（理由は error ログに出る）、残りの同梱フォントには影響しない。
    #[test]
    fn from_game_font_bytes_skips_cff_font_without_affecting_others() {
        let mut otto = b"OTTO".to_vec();
        otto.extend_from_slice(&[0u8; 64]);
        let good = super::super::tests::minimal_font_bytes("Game Good Font");
        let reg = FontRegistry::from_game_font_bytes(&[otto, good]);
        assert_eq!(reg.len(), 1);
        assert!(reg.resolve("Game Good Font").is_some());
    }

    /// TTC（複数フォント入り）も従来どおりメモリ経由で全フォント索引される。
    #[test]
    fn from_game_font_bytes_indexes_all_fonts_in_ttc() {
        let a = super::super::tests::minimal_font_bytes("Game TTC A");
        let b = super::super::tests::minimal_font_bytes("Game TTC B");
        let ttc = super::super::tests::wrap_as_ttc_for_test(&[a, b]);
        let reg = FontRegistry::from_game_font_bytes(&[ttc]);
        assert_eq!(reg.len(), 2);
        assert!(reg.resolve("Game TTC A").is_some());
        assert!(reg.resolve("Game TTC B").is_some());
    }
}
