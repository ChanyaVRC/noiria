//! NPK1 — 自作の暗号化 pak フォーマット（W6: 配布対応）。
//!
//! ## 脅威モデルとスコープ
//! ChaCha20 (RFC 8439, `crypto` モジュール) でエントリ本体とインデックス（＝ファイル名一覧）を
//! 両方暗号化する。**Poly1305 等の MAC は実装しない**（ユーザー承認済みのスコープ外判断）。
//! 想定脅威は「エクスプローラでゲームフォルダを開いて画像/音声を気軽に抜き出す」程度の
//! カジュアルな抜き出し抑止であり、意図的な改ざんに対する耐タンパー性は目的にしていない。
//! 整合性検査は各エントリの CRC32（展開後の平文に対して計算）で十分とする —
//! 転送/ディスク破損の検出が目的であり、暗号学的な改ざん検知ではない。
//!
//! ## ファイルレイアウト
//! ```text
//! [ヘッダ 48 バイト][エントリ本体（圧縮+暗号化）を連結][インデックス（圧縮+暗号化）]
//! ```
//! すべての複数バイト整数はリトルエンディアン。
//!
//! ### ヘッダ (48 バイト固定)
//! - `magic: [u8; 4]` = `b"NPK1"`
//! - `version: u16` = 1
//! - `flags: u16` = 0（予約）
//! - `key_check: [u8; 8]` = `ChaCha20(key, nonce=[0;12], counter=0)` キーストリームの先頭 8 バイト
//!   （平文は関与しない、鍵の妥当性だけを見る値）。open() 時に再計算して不一致なら「鍵が違う」と
//!   明確に報告する。
//! - `index_ofs: u64` / `index_len: u64`（ディスク上サイズ）/ `index_plain_len: u64`（展開後サイズ）
//! - `index_crc32: u32`（オフセット 40）= 展開後（平文）インデックスの CRC32。**0 は「記録なし」**
//!   （この欄ができる前の pak。予約領域がゼロだったので、検査を飛ばして従来どおり開く）。
//!   値が入っていれば open() が検査し、不一致は「壊れた pak」として拒否する。
//! - 残り 4 バイトは予約（ゼロ埋め）
//!
//! ### エントリ本体
//! 各ファイルは「(必要なら) DEFLATE 圧縮 → ChaCha20 暗号化」した後、ヘッダ直後から連続して
//! 詰める（パディングなし。境界は各エントリの `offset`/`stored_len` で管理する）。
//!
//! ### インデックス
//! エントリテーブルを独自のコンパクトなバイナリ形式（後述）でシリアライズし、
//! 生 DEFLATE (`codec::deflate::deflate_compress`) で圧縮してから ChaCha20 で暗号化する
//! （ファイル名一覧を秘匿するため）。**zlib ラッパではなく raw DEFLATE を使う**理由:
//! エントリ・インデックスとも既に CRC32 で整合性を見ているため、zlib の adler32 ヘッダは
//! 冗長なオーバーヘッドでしかない（`docs/implementation-notes.md` 2026-07-12 W6 参照）。
//!
//! インデックスのバイナリ形式（`u32` エントリ数 + 各エントリ）:
//! `path_len: u16` + `path` (UTF-8, `/` 区切り, 相対パス) + `offset: u64` + `stored_len: u64` +
//! `plain_len: u64` + `codec: u8` + `crc32: u32` + `entry_id: u64`。
//!
//! ### nonce 設計
//! 12 バイト = 4 バイト固定タグ `b"NPK1"` + 8 バイト `entry_id` (LE)。カウンタは各エントリ
//! （インデックスも含む）ごとに 0 から始まる（エントリ内バイトオフセット `ofs` は
//! `counter = ofs/64`, ブロック内オフセット `ofs%64` に対応する）。これにより
//! `PakEntryReader` は先頭から復号せずに任意オフセットへシークできる。
//! インデックス自身は実エントリ（`0..N`）と衝突しない `entry_id = u64::MAX` を使う。
//! ヘッダの `key_check` だけは nonce 全ゼロ固定（entry_id 由来ではない別枠の検査値）。
//!
//! ### コーデック
//! - `0` = 無圧縮 (stored)
//! - `1` = 生 DEFLATE
//! - `2` = wav-flac（W9: FLAC 風サブセットコーデック。WAV の data チャンクを可逆圧縮した
//!   独自バイト列。`codec::flac_subset::decode_wav` が元 WAV バイト列を完全復元する。
//!   エンコーダは `noiria-tools::flac_enc`（意図的に別クレートで独立実装、W4 の教訓:
//!   対称な実装は round-trip のバグを隠す）。選択は `noiria-tools::pack::pack_dir` が
//!   `build_with_special_codec` の hook 経由で行う — `pak.rs` (noiria-core) は
//!   `noiria-tools` に依存できないため encoder を直接呼べない設計上の制約による）

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
#[cfg(not(any(unix, windows)))]
use std::sync::Mutex;

use anyhow::{bail, ensure, Context, Result};

const MAGIC: &[u8; 4] = b"NPK1";
const VERSION: u16 = 1;
const HEADER_LEN: usize = 48;

pub const CODEC_STORED: u8 = 0;
pub const CODEC_DEFLATE: u8 = 1;
/// W9: wav-flac（FLAC 風サブセットコーデック、`codec::flac_subset` 参照）。
pub const CODEC_WAV_FLAC: u8 = 2;

const INDEX_ENTRY_ID: u64 = u64::MAX;

/// 拡張子ベースで「pack 時に圧縮を試みるか」を決める純粋関数。既に圧縮済みの
/// フォーマット（画像/音声/動画コンテナ）は無圧縮 (stored) にする（再圧縮しても
/// ほぼ縮まらず CPU の無駄）。それ以外はこの後 DEFLATE を試して、縮まらなければ
/// 呼び出し側 (`build`) が stored にフォールバックする。
pub fn is_precompressed_ext(path: &str) -> bool {
    const EXTS: &[&str] = &["png", "jpg", "jpeg", "ogg", "avi", "ivf", "mp4", "webp"];
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    EXTS.contains(&ext.as_str())
}

fn entry_nonce(entry_id: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[0..4].copy_from_slice(MAGIC);
    n[4..12].copy_from_slice(&entry_id.to_le_bytes());
    n
}

fn key_check_value(key: &[u8; 32]) -> [u8; 8] {
    let ks = crate::crypto::block(key, &[0u8; 12], 0);
    [ks[0], ks[1], ks[2], ks[3], ks[4], ks[5], ks[6], ks[7]]
}

// ---------------------------------------------------------------------
// 書き込み (pack)
// ---------------------------------------------------------------------

/// pack 結果のレポート（`noiria-pack` CLI の進捗表示用）。
#[derive(Debug, Clone, Default)]
pub struct PackReport {
    pub entry_count: usize,
    pub codec_stored_count: usize,
    pub codec_deflate_count: usize,
    pub total_plain_len: u64,
    pub total_pak_len: u64,
    /// W8: PNG→WebP ロスレス変換で実際に WebP の方を採用した件数
    /// （`noiria_tools::pack::pack_dir` が `build()` の後にセットする。
    /// `build()` 自体は画像フォーマットを意識しない）。
    pub webp_converted_count: usize,
    /// W8: 上記の採用によって削減できた合計バイト数（元 PNG − 採用後サイズの総和）。
    pub webp_bytes_saved: u64,
    /// W9: WAV→wav-flac (codec=2) 変換を実際に採用した件数。webp と異なり、これは
    /// `build_with_special_codec` の hook 経由で `build()` 相当の処理内で直接わかる
    /// （`plain`＝元 WAV バイト列と `stored`＝flac blob の両方を同時に見られるため）。
    pub flac_converted_count: usize,
    /// W9: 上記の採用によって削減できた合計バイト数（元 WAV − flac blob サイズの総和）。
    pub flac_bytes_saved: u64,
}

#[derive(Clone)]
struct IndexEntry {
    offset: u64,
    stored_len: u64,
    plain_len: u64,
    codec: u8,
    crc32: u32,
    entry_id: u64,
}

/// 拡張子ルールに従って 1 エントリを圧縮する。stored を選んだ場合でも
/// DEFLATE を 1 度試して縮まなければ stored にフォールバックする（既定拡張子のみ）。
fn compress_for_entry(path: &str, data: &[u8]) -> (u8, Vec<u8>) {
    if is_precompressed_ext(path) {
        return (CODEC_STORED, data.to_vec());
    }
    let compressed = crate::codec::deflate::deflate_compress(data);
    if compressed.len() < data.len() {
        (CODEC_DEFLATE, compressed)
    } else {
        (CODEC_STORED, data.to_vec())
    }
}

fn write_index_entry(out: &mut Vec<u8>, path: &str, e: &IndexEntry) {
    let path_bytes = path.as_bytes();
    out.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
    out.extend_from_slice(path_bytes);
    out.extend_from_slice(&e.offset.to_le_bytes());
    out.extend_from_slice(&e.stored_len.to_le_bytes());
    out.extend_from_slice(&e.plain_len.to_le_bytes());
    out.push(e.codec);
    out.extend_from_slice(&e.crc32.to_le_bytes());
    out.extend_from_slice(&e.entry_id.to_le_bytes());
}

fn build_header(
    key: &[u8; 32],
    index_ofs: u64,
    index_len: u64,
    index_plain_len: u64,
    index_crc32: u32,
) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[0..4].copy_from_slice(MAGIC);
    h[4..6].copy_from_slice(&VERSION.to_le_bytes());
    h[6..8].copy_from_slice(&0u16.to_le_bytes()); // flags
    h[8..16].copy_from_slice(&key_check_value(key));
    h[16..24].copy_from_slice(&index_ofs.to_le_bytes());
    h[24..32].copy_from_slice(&index_len.to_le_bytes());
    h[32..40].copy_from_slice(&index_plain_len.to_le_bytes());
    h[40..44].copy_from_slice(&index_crc32.to_le_bytes());
    // 44..48 は予約、ゼロ埋めのまま
    h
}

/// `(相対パス, 平文バイト列)` の一覧から NPK1 pak バイト列を構築する。
/// `noiria-tools` の `pack::pack_dir` から呼ばれる。拡張子ベースの stored/DEFLATE
/// 自動選択のみを行う（`build_with_special_codec(entries, key, |_, _| None)` の薄いラッパ）。
pub fn build(entries: Vec<(String, Vec<u8>)>, key: &[u8; 32]) -> (Vec<u8>, PackReport) {
    build_with_special_codec(entries, key, |_, _| None)
}

/// `build` の一般化版。各エントリに対して先に `special(path, plain)` を呼び出し、
/// `Some((codec, stored_bytes))` が返ればそれをそのまま採用する（`stored_bytes` は
/// 追加の DEFLATE 圧縮をせずそのまま格納する。呼び出し側が既にエンコード済みという契約）。
/// `None` の場合は従来どおり拡張子ベースの stored/DEFLATE 自動選択
/// (`compress_for_entry`) にフォールバックする。
///
/// W9 で追加: `noiria-tools::pack_dir` が WAV→wav-flac (codec=2) 変換をここへ差し込むために
/// 使う。`pak.rs` (noiria-core) は `noiria-tools` に依存できない（依存方向が逆）ため、
/// FLAC-subset エンコーダを直接呼べない — hook 経由で呼び出し側にエンコード処理を
/// 委ねる設計にした。`special` は `CODEC_STORED`/`CODEC_DEFLATE`/`CODEC_WAV_FLAC` の
/// いずれかだけを返す契約（それ以外の値を返すと `unreachable!` になる）。
pub fn build_with_special_codec(
    entries: Vec<(String, Vec<u8>)>,
    key: &[u8; 32],
    mut special: impl FnMut(&str, &[u8]) -> Option<(u8, Vec<u8>)>,
) -> (Vec<u8>, PackReport) {
    // ヘッダ分の 48 バイトを空けておき、エントリ本体はその後ろへ直接追記する（別の Vec に
    // 溜めてから最後にコピーするとピークメモリが pak の約 2 倍になる）。ヘッダは最後に埋める。
    let mut out = vec![0u8; HEADER_LEN];
    let mut index_entries: Vec<(String, IndexEntry)> = Vec::with_capacity(entries.len());
    let mut codec_stored_count = 0usize;
    let mut codec_deflate_count = 0usize;
    let mut codec_wav_flac_count = 0usize;
    let mut flac_bytes_saved = 0u64;
    let mut total_plain_len = 0u64;

    for (entry_id, (path, plain)) in entries.into_iter().enumerate() {
        let entry_id = entry_id as u64;
        total_plain_len += plain.len() as u64;
        let crc = crate::codec::png::crc32(&plain);
        let (codec, stored_bytes) = match special(&path, &plain) {
            Some(r) => r,
            None => compress_for_entry(&path, &plain),
        };
        match codec {
            CODEC_STORED => codec_stored_count += 1,
            CODEC_DEFLATE => codec_deflate_count += 1,
            CODEC_WAV_FLAC => {
                codec_wav_flac_count += 1;
                flac_bytes_saved += plain.len().saturating_sub(stored_bytes.len()) as u64;
            }
            _ => unreachable!("special/compress_for_entry は 0/1/2 以外を返さない"),
        }
        let mut cipher = stored_bytes;
        let nonce = entry_nonce(entry_id);
        crate::crypto::apply_keystream(key, &nonce, 0, &mut cipher);
        let offset = out.len() as u64;
        let stored_len = cipher.len() as u64;
        out.extend_from_slice(&cipher);
        index_entries.push((
            path,
            IndexEntry {
                offset,
                stored_len,
                plain_len: plain.len() as u64,
                codec,
                crc32: crc,
                entry_id,
            },
        ));
    }

    let mut index_plain = Vec::new();
    index_plain.extend_from_slice(&(index_entries.len() as u32).to_le_bytes());
    for (path, e) in &index_entries {
        write_index_entry(&mut index_plain, path, e);
    }
    let index_plain_len = index_plain.len() as u64;
    let index_crc32 = crate::codec::png::crc32(&index_plain);
    let mut index_cipher = crate::codec::deflate::deflate_compress(&index_plain);
    let index_nonce = entry_nonce(INDEX_ENTRY_ID);
    crate::crypto::apply_keystream(key, &index_nonce, 0, &mut index_cipher);
    let index_ofs = out.len() as u64;
    let index_len = index_cipher.len() as u64;

    out.extend_from_slice(&index_cipher);
    out[..HEADER_LEN].copy_from_slice(&build_header(
        key,
        index_ofs,
        index_len,
        index_plain_len,
        index_crc32,
    ));

    let report = PackReport {
        entry_count: index_entries.len(),
        codec_stored_count,
        codec_deflate_count,
        total_plain_len,
        total_pak_len: out.len() as u64,
        // WebP 変換は `build()` の呼び出し側 (`noiria_tools::pack::pack_dir`) が
        // entries を差し替えた後にここへ渡すため、`build()` 自体は関知しない。
        webp_converted_count: 0,
        webp_bytes_saved: 0,
        flac_converted_count: codec_wav_flac_count,
        flac_bytes_saved,
    };
    (out, report)
}

// ---------------------------------------------------------------------
// 読み込みバイナリカーソル（index パース用）
// ---------------------------------------------------------------------

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(
            self.pos + n <= self.data.len(),
            "index が壊れています（範囲外読み取り）"
        );
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn take_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let slice = self.take(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take_array()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take_array()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take_array()?))
    }
}

/// インデックス 1 エントリの最小バイト数（空のパス）: path_len(2) + offset/stored_len/plain_len(24)
/// + codec(1) + crc32(4) + entry_id(8)。
const MIN_INDEX_ENTRY_LEN: usize = 2 + 8 + 8 + 8 + 1 + 4 + 8;

/// 展開後インデックスの最大サイズ（ヘッダの申告値をそのまま信じて確保しないための上限）。
const MAX_INDEX_PLAIN_LEN: u64 = 1 << 30;

fn parse_index(data: &[u8]) -> Result<HashMap<String, IndexEntry>> {
    let mut r = Cursor::new(data);
    let count = r.u32()? as usize;
    // 件数はファイル由来の u32。実際に入りうる件数（1 エントリ最小 MIN_INDEX_ENTRY_LEN バイト）で
    // 事前確保を抑える。
    let mut map = HashMap::with_capacity(count.min(data.len() / MIN_INDEX_ENTRY_LEN));
    for _ in 0..count {
        let path_len = r.u16()? as usize;
        let path_bytes = r.take(path_len)?;
        let path = String::from_utf8(path_bytes.to_vec())
            .context("index のパスが UTF-8 ではありません")?;
        let offset = r.u64()?;
        let stored_len = r.u64()?;
        let plain_len = r.u64()?;
        let codec = r.u8()?;
        let crc32 = r.u32()?;
        let entry_id = r.u64()?;
        map.insert(
            path,
            IndexEntry {
                offset,
                stored_len,
                plain_len,
                codec,
                crc32,
                entry_id,
            },
        );
    }
    Ok(map)
}

// ---------------------------------------------------------------------
// 読み込み (Pak)
// ---------------------------------------------------------------------

enum PakSource {
    Memory(Vec<u8>),
    File {
        #[cfg(any(unix, windows))]
        file: File,
        #[cfg(not(any(unix, windows)))]
        file: Mutex<File>,
        len: u64,
    },
}

impl PakSource {
    fn from_file(file: File, len: u64) -> Self {
        Self::File {
            #[cfg(any(unix, windows))]
            file,
            #[cfg(not(any(unix, windows)))]
            file: Mutex::new(file),
            len,
        }
    }

    fn len(&self) -> u64 {
        match self {
            Self::Memory(data) => data.len() as u64,
            Self::File { len, .. } => *len,
        }
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        match self {
            Self::Memory(data) => {
                let Ok(start) = usize::try_from(offset) else {
                    return Ok(0);
                };
                if start >= data.len() {
                    return Ok(0);
                }
                let n = buf.len().min(data.len() - start);
                buf[..n].copy_from_slice(&data[start..start + n]);
                Ok(n)
            }
            Self::File { file, .. } => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::FileExt;
                    loop {
                        match file.read_at(buf, offset) {
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            other => return other,
                        }
                    }
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::FileExt;
                    // seek_read はファイルポインタも更新するが、全ての読み出しが明示オフセットを
                    // 渡しカーネルが同期ハンドルの排他を保つため並行呼び出しでも干渉しない。
                    loop {
                        match file.seek_read(buf, offset) {
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            other => return other,
                        }
                    }
                }
                #[cfg(not(any(unix, windows)))]
                {
                    let mut f = file.lock().unwrap_or_else(|e| e.into_inner());
                    f.seek(SeekFrom::Start(offset))?;
                    loop {
                        match f.read(buf) {
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            other => return other,
                        }
                    }
                }
            }
        }
    }

    fn read_exact_at(&self, mut offset: u64, mut buf: &mut [u8]) -> std::io::Result<()> {
        while !buf.is_empty() {
            let n = self.read_at(offset, buf)?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "pak ファイル終端に到達しました",
                ));
            }
            offset = offset.checked_add(n as u64).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "pak オフセットがオーバーフローしました",
                )
            })?;
            buf = &mut buf[n..];
        }
        Ok(())
    }
}

/// 開いた NPK1 pak。`Pak::open` はメモリ上のバイト列、`Pak::open_file` はファイルハンドルから
/// 必要な範囲だけをオンデマンドで読み出す（アーカイブ全体を RAM に常駐させない）。
pub struct Pak {
    source: Arc<PakSource>,
    key: [u8; 32],
    entries: HashMap<String, IndexEntry>,
}

impl Pak {
    /// pak バイト列と鍵から開く。マジック/バージョン/鍵検証を行い、インデックスを
    /// 復号・展開・パースする。鍵が違う場合は明確なエラーを返す（パースエラーと区別できる）。
    pub fn open(data: Vec<u8>, key: &[u8; 32]) -> Result<Self> {
        Self::open_source(PakSource::Memory(data), key)
    }

    /// pak ファイルと鍵から開く。ヘッダとインデックス領域だけを読み込んで検証・パースし、
    /// 各エントリ本体は `read` / `entry_reader` 呼び出し時に必要な範囲だけを読む。
    pub fn open_file(path: impl AsRef<Path>, key: &[u8; 32]) -> Result<Self> {
        let path = path.as_ref();
        let file = File::open(path)
            .with_context(|| format!("pak ファイルが開けません: {}", path.display()))?;
        let len = file
            .metadata()
            .with_context(|| {
                format!(
                    "pak ファイルのメタデータ取得に失敗しました: {}",
                    path.display()
                )
            })?
            .len();
        Self::open_source(PakSource::from_file(file, len), key)
    }

    fn open_source(source: PakSource, key: &[u8; 32]) -> Result<Self> {
        ensure!(
            source.len() >= HEADER_LEN as u64,
            "pak ファイルが小さすぎます（ヘッダが不足しています）"
        );
        let mut header = [0u8; HEADER_LEN];
        source
            .read_exact_at(0, &mut header)
            .context("pak ヘッダの読み込みに失敗しました")?;
        ensure!(
            &header[0..4] == MAGIC,
            "NPK1 形式ではありません（マジックバイト不一致）"
        );
        let version = u16::from_le_bytes([header[4], header[5]]);
        ensure!(
            version == VERSION,
            "対応していない pak バージョンです: {version}"
        );

        let expected = key_check_value(key);
        ensure!(
            header[8..16] == expected,
            "鍵が違います。この pak を復号できません（key_check 不一致）"
        );

        let mut hdr_cur = Cursor::new(&header[16..44]);
        let index_ofs = hdr_cur.u64()?;
        let index_len = hdr_cur.u64()?;
        let index_plain_len = hdr_cur.u64()?;
        let index_crc32 = hdr_cur.u32()?;
        let index_end = index_ofs
            .checked_add(index_len)
            .context("index の範囲指定が不正です")?;
        ensure!(
            index_end <= source.len(),
            "index がファイル範囲外を指しています（壊れた pak）"
        );
        ensure!(
            index_plain_len <= MAX_INDEX_PLAIN_LEN,
            "index の展開後サイズが大きすぎます（壊れた pak）"
        );
        let index_len_usize = usize::try_from(index_len).context("index の範囲指定が不正です")?;

        let mut index_cipher = vec![0u8; index_len_usize];
        source
            .read_exact_at(index_ofs, &mut index_cipher)
            .context("index の読み込みに失敗しました（壊れた pak）")?;
        let index_nonce = entry_nonce(INDEX_ENTRY_ID);
        crate::crypto::apply_keystream(key, &index_nonce, 0, &mut index_cipher);
        let index_plain =
            crate::codec::inflate::inflate_limited(&index_cipher, index_plain_len as usize)
                .context("index の展開に失敗しました（壊れた pak）")?;
        ensure!(
            index_plain.len() as u64 == index_plain_len,
            "index の展開後サイズが一致しません（壊れた pak）"
        );
        ensure!(
            index_crc32 == 0 || crate::codec::png::crc32(&index_plain) == index_crc32,
            "index の CRC32 が一致しません（壊れた pak）"
        );

        let entries = parse_index(&index_plain)?;
        Ok(Self {
            source: Arc::new(source),
            key: *key,
            entries,
        })
    }

    pub fn exists(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    /// エントリが実際にどの codec (`CODEC_STORED`/`CODEC_DEFLATE`/`CODEC_WAV_FLAC`) で
    /// 格納されているかを返す（テスト・CLI 診断用。エンジン本体は使わない）。
    pub fn codec_of(&self, path: &str) -> Option<u8> {
        self.entries.get(path).map(|e| e.codec)
    }

    /// `prefix/` 直下のファイル名一覧（1 階層のみ）。`GameDir::list_dir` と同じ契約
    /// （該当エントリが 0 件でもエラーにせず空 Vec を返す）。
    pub fn list_dir(&self, prefix: &str) -> Vec<String> {
        let pattern = if prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", prefix.trim_end_matches('/'))
        };
        let mut names: Vec<String> = self
            .entries
            .keys()
            .filter_map(|p| {
                let rest = p.strip_prefix(pattern.as_str())?;
                if rest.is_empty() || rest.contains('/') {
                    None
                } else {
                    Some(rest.to_string())
                }
            })
            .collect();
        names.sort();
        names
    }

    /// 1 エントリを全復号・全展開して返す。展開後 CRC32 を検証する。
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let e = self
            .entries
            .get(path)
            .with_context(|| format!("pak にエントリがありません: {path}"))?;
        let end = e
            .offset
            .checked_add(e.stored_len)
            .context("pak エントリのサイズが不正です")?;
        ensure!(
            end <= self.source.len(),
            "pak エントリの範囲がファイル外です: {path}"
        );
        let stored_len = usize::try_from(e.stored_len).context("pak エントリのサイズが不正です")?;

        let mut buf = vec![0u8; stored_len];
        self.source
            .read_exact_at(e.offset, &mut buf)
            .with_context(|| format!("pak エントリの読み込みに失敗しました: {path}"))?;
        let nonce = entry_nonce(e.entry_id);
        crate::crypto::apply_keystream(&self.key, &nonce, 0, &mut buf);

        // 索引が申告する展開後サイズを超えて展開させない（圧縮爆弾対策）。
        let plain_limit = usize::try_from(e.plain_len).unwrap_or(usize::MAX);
        let plain = match e.codec {
            CODEC_STORED => buf,
            CODEC_DEFLATE => crate::codec::inflate::inflate_limited(&buf, plain_limit)
                .with_context(|| format!("pak エントリの展開に失敗しました: {path}"))?,
            CODEC_WAV_FLAC => crate::codec::flac_subset::decode_wav_limited(&buf, plain_limit)
                .with_context(|| format!("pak エントリの wav-flac 展開に失敗しました: {path}"))?,
            other => bail!("未対応の codec です ({other}): {path}"),
        };
        ensure!(
            plain.len() as u64 == e.plain_len,
            "pak エントリの展開後サイズが不一致です（破損の可能性）: {path}"
        );
        let crc = crate::codec::png::crc32(&plain);
        ensure!(
            crc == e.crc32,
            "pak エントリの CRC32 が不一致です（破損の可能性）: {path}"
        );
        Ok(plain)
    }

    fn entry_cursor(&self, path: &str) -> Result<EntryCursor> {
        let e = self
            .entries
            .get(path)
            .with_context(|| format!("pak にエントリがありません: {path}"))?;
        ensure!(
            e.codec == CODEC_STORED,
            "PakEntryReader は無圧縮 (codec=0) エントリのみ対応です（{path} は codec={}）",
            e.codec
        );
        ensure!(
            e.plain_len == e.stored_len,
            "pak エントリのサイズが矛盾しています（stored={} plain={}）: {path}",
            e.stored_len,
            e.plain_len
        );
        let end = e
            .offset
            .checked_add(e.stored_len)
            .context("pak エントリのサイズが不正です")?;
        ensure!(
            end <= self.source.len(),
            "pak エントリの範囲がファイル外です: {path}"
        );
        Ok(EntryCursor {
            base_offset: e.offset,
            stored_len: e.stored_len,
            key: self.key,
            nonce: entry_nonce(e.entry_id),
            len: e.plain_len,
            pos: 0,
        })
    }

    /// 無圧縮 (codec=0) エントリをシーク可能に読むリーダーを作る。ChaCha20 のカウンタ
    /// シーク性を使い、先頭から復号せずに任意オフセットの読み出しができる。
    /// 圧縮エントリ (codec=1) には使えない（圧縮ストリームは途中から展開できないため）。
    pub fn entry_reader(&self, path: &str) -> Result<PakEntryReader<'_>> {
        let cursor = self.entry_cursor(path)?;
        Ok(PakEntryReader {
            source: &self.source,
            cursor,
        })
    }

    /// 無圧縮 (codec=0) エントリを所有権付き (`'static + Send`) で読むリーダーを作る。
    /// 音声ストリーミングなど `Pak` の借用をまたいで保持する用途向け。
    pub fn open_entry_reader(&self, path: &str) -> Result<OwnedPakEntryReader> {
        let cursor = self.entry_cursor(path)?;
        Ok(OwnedPakEntryReader {
            source: Arc::clone(&self.source),
            cursor,
        })
    }
}

struct EntryCursor {
    base_offset: u64,
    stored_len: u64,
    key: [u8; 32],
    nonce: [u8; 12],
    len: u64,
    pos: u64,
}

impl EntryCursor {
    fn read(&mut self, source: &PakSource, buf: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.len.min(self.stored_len).saturating_sub(self.pos);
        if remaining == 0 || buf.is_empty() {
            return Ok(0);
        }
        let to_read = (buf.len() as u64).min(remaining) as usize;
        let file_offset = self.base_offset.checked_add(self.pos).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "pak オフセットがオーバーフローしました",
            )
        })?;
        let n = source.read_at(file_offset, &mut buf[..to_read])?;
        if n == 0 {
            return Ok(0);
        }
        crate::crypto::apply_keystream(&self.key, &self.nonce, self.pos, &mut buf[..n]);
        self.pos += n as u64;
        Ok(n)
    }

    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(off) => self.len.checked_add_signed(off),
            SeekFrom::Current(off) => self.pos.checked_add_signed(off),
        };
        let Some(new_pos) = new_pos else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "シーク位置が不正です",
            ));
        };
        self.pos = new_pos;
        Ok(self.pos)
    }
}

/// `Pak` の 1 エントリ（無圧縮のみ）をシーク可能に読み出すリーダー。ChaCha20 のカウンタは
/// エントリ内バイトオフセットから直接導出するので、`seek()` しても先頭からの再復号は不要。
pub struct PakEntryReader<'a> {
    source: &'a PakSource,
    cursor: EntryCursor,
}

impl Read for PakEntryReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.cursor.read(self.source, buf)
    }
}

impl Seek for PakEntryReader<'_> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(pos)
    }
}

/// `PakEntryReader` の所有権付き (`'static + Send`) 版。
pub struct OwnedPakEntryReader {
    source: Arc<PakSource>,
    cursor: EntryCursor,
}

impl OwnedPakEntryReader {
    pub fn len(&self) -> u64 {
        self.cursor.len
    }

    pub fn is_empty(&self) -> bool {
        self.cursor.len == 0
    }
}

impl Read for OwnedPakEntryReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.cursor.read(&self.source, buf)
    }
}

impl Seek for OwnedPakEntryReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(pos)
    }
}

// ---------------------------------------------------------------------
// 鍵管理
// ---------------------------------------------------------------------

/// `KEY_SLOT` 先頭 16 バイトのマジック（"NOIRIA-KEYSLOT1" + NUL 終端で正確に 16 バイト）。
/// W7（本 wave の対象外）がコンパイル済み実行ファイルをこのマジックで探し、後続 32 バイトを
/// 実鍵にバイナリパッチする想定の目印。
const KEY_SLOT_MAGIC: [u8; 16] = *b"NOIRIA-KEYSLOT1\0";

/// `KEY_SLOT_MAGIC` を公開する（W7 `noiria-export` がビルド済み実行ファイル中から
/// このマジックをバイト検索して後続 32 バイトを鍵でパッチするために使う）。
pub fn key_slot_magic() -> [u8; 16] {
    KEY_SLOT_MAGIC
}

/// 実行ファイルにバイナリパッチで鍵を埋め込むための領域（W7 の下準備、本 wave では未パッチ）。
/// `#[used]` を付けて、コンパイラ最適化（LTO 等）で「どこからも読まれない全ゼロ static」として
/// 消去・重複排除されないようにしている。`static`（`const` ではない）にしているのも同じ理由:
/// `const` は使用箇所へ値がインライン展開されるため、パッチ対象となる実体（固定アドレスの
/// メモリ領域）が実行ファイル中に残らない。
#[used]
static KEY_SLOT: [u8; 48] = {
    let mut arr = [0u8; 48];
    let mut i = 0;
    while i < 16 {
        arr[i] = KEY_SLOT_MAGIC[i];
        i += 1;
    }
    arr
    // 17..48 (鍵領域) は未パッチ状態としてゼロのまま。
};

/// `KEY_SLOT` の鍵領域を読む。全ゼロ（未パッチ）なら `None`。
/// `ptr::read_volatile` で 1 バイトずつ読むのは、コンパイラが「初期値は全ゼロと分かっている
/// static だから `key_slot_key()` は定数畳み込みで常に `None` を返す」と最適化してしまうのを
/// 防ぐため（W7 のバイナリパッチはコンパイラの知らないところで実行ファイルを書き換えるため、
/// 通常の最適化はこの前提を壊しうる）。
fn key_slot_key() -> Option<[u8; 32]> {
    let mut bytes = [0u8; 48];
    // SAFETY: base は 48 バイトの static 配列の先頭で、i は 0..48 の範囲なので `base.add(i)` は
    // 配列内を指す。読み出しのみで、書き込み（バイナリパッチ）とは並行しない（起動時の鍵解決のみ）。
    unsafe {
        let base = KEY_SLOT.as_ptr();
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = std::ptr::read_volatile(base.add(i));
        }
    }
    let key_region = &bytes[16..48];
    if key_region.iter().all(|&b| b == 0) {
        None
    } else {
        let mut key = [0u8; 32];
        key.copy_from_slice(key_region);
        Some(key)
    }
}

/// 未パッチ実行ファイル（開発ビルド）が使う既定鍵。秘密ではない（`docs/implementation-notes.md`
/// 2026-07-12 W6 に平文の hex 値を記載済み）。「配布用に鍵を差し替える」W7 のバイナリパッチが
/// 適用されるまでのプレースホルダ。
const DEV_KEY: [u8; 32] = *b"Noiria-W6-DevKey-2026-07-12!!!!!";

/// 64 桁の 16 進文字列（32 バイト）を鍵に変換する。`NOIRIA_PAK_KEY` と `noiria-pack --key` 用。
/// バイト列として 2 桁ずつ読み、16 進数字（0-9 a-f A-F）以外は `+`・`-`・空白・マルチバイト文字を
/// 含めて拒否する（文字列スライスや `from_str_radix` は境界で panic したり `+f` を受理する）。
pub fn parse_hex_key(hex: &str) -> Result<[u8; 32]> {
    let bytes = hex.as_bytes();
    ensure!(
        bytes.len() == 64,
        "NOIRIA_PAK_KEY は 64 桁の16進数（32バイト）である必要があります（実際: {} バイト）",
        bytes.len()
    );
    fn nibble(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let mut key = [0u8; 32];
    for (i, out) in key.iter_mut().enumerate() {
        match (nibble(bytes[i * 2]), nibble(bytes[i * 2 + 1])) {
            (Some(hi), Some(lo)) => *out = (hi << 4) | lo,
            _ => bail!(
                "NOIRIA_PAK_KEY の {} 桁目付近に16進数字ではない文字があります",
                i * 2 + 1
            ),
        }
    }
    Ok(key)
}

/// エンジン (`GameDir::open`) が pak を開く際の鍵解決。優先順位:
/// 1. `KEY_SLOT` がバイナリパッチ済み（全ゼロでない）ならそれを使う
/// 2. 環境変数 `NOIRIA_PAK_KEY`（64桁 hex）があればそれを使う
/// 3. どちらも無ければ `DEV_KEY`（開発用固定鍵）
pub fn resolve_engine_key() -> Result<[u8; 32]> {
    if let Some(k) = key_slot_key() {
        return Ok(k);
    }
    if let Ok(hex) = std::env::var("NOIRIA_PAK_KEY") {
        return parse_hex_key(&hex);
    }
    Ok(DEV_KEY)
}

/// `noiria-pack` CLI が `--key` 省略時に使う既定鍵（`DEV_KEY` と同じ値を公開）。
pub fn dev_key() -> [u8; 32] {
    DEV_KEY
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        [0x42u8; 32]
    }

    #[test]
    fn roundtrip_basic() {
        let entries = vec![
            ("game.json".to_string(), br#"{"title":"t"}"#.to_vec()),
            (
                "scenario/first.ks".to_string(),
                b"@title\n[cm]hello world[cm]".to_vec(),
            ),
            (
                "assets/bg/room.png".to_string(),
                vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10],
            ),
        ];
        let key = test_key();
        let (bytes, report) = build(entries.clone(), &key);
        assert_eq!(report.entry_count, 3);

        let pak = Pak::open(bytes, &key).unwrap();
        for (path, plain) in &entries {
            assert!(pak.exists(path));
            assert_eq!(&pak.read(path).unwrap(), plain, "path={path}");
        }
        assert!(!pak.exists("nope.txt"));
    }

    #[test]
    fn wrong_key_is_rejected_clearly() {
        let entries = vec![("a.txt".to_string(), b"hello".to_vec())];
        let (bytes, _) = build(entries, &test_key());
        let wrong_key = [0x99u8; 32];
        let err = match Pak::open(bytes, &wrong_key) {
            Ok(_) => panic!("誤った鍵で開けてしまった"),
            Err(e) => e,
        };
        let msg = format!("{err:#}");
        assert!(msg.contains("鍵が違います"), "{msg}");
    }

    #[test]
    fn codec_selection_stores_precompressed_and_deflates_text() {
        let text = "same text repeated ".repeat(200);
        let entries = vec![
            ("bg/room.png".to_string(), vec![0xAAu8; 5000]), // 拡張子で stored 強制
            ("scenario/x.ks".to_string(), text.clone().into_bytes()), // deflate が効くはず
        ];
        let key = test_key();
        let (bytes, report) = build(entries, &key);
        assert_eq!(report.codec_stored_count, 1);
        assert_eq!(report.codec_deflate_count, 1);
        assert!(report.total_pak_len < HEADER_LEN as u64 + 5000 + text.len() as u64);

        let pak = Pak::open(bytes, &key).unwrap();
        assert_eq!(pak.read("scenario/x.ks").unwrap(), text.into_bytes());
    }

    #[test]
    fn index_and_content_are_not_present_as_plaintext() {
        let entries = vec![
            (
                "first.ks".to_string(),
                b"a very unique scenario body xyz123".to_vec(),
            ),
            ("fg/alice_normal.png".to_string(), vec![7u8; 4096]),
        ];
        let key = test_key();
        let (bytes, _) = build(entries, &key);
        let hay = &bytes;
        assert!(!contains_subslice(hay, b"first.ks"));
        assert!(!contains_subslice(hay, b"alice_normal.png"));
        assert!(!contains_subslice(
            hay,
            b"a very unique scenario body xyz123"
        ));
    }

    fn contains_subslice(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn list_dir_matches_one_level_contract() {
        let entries = vec![
            ("fonts/a.ttf".to_string(), b"a".to_vec()),
            ("fonts/b.ttc".to_string(), b"b".to_vec()),
            ("fonts/nested/c.ttf".to_string(), b"c".to_vec()),
            ("other.txt".to_string(), b"o".to_vec()),
        ];
        let key = test_key();
        let (bytes, _) = build(entries, &key);
        let pak = Pak::open(bytes, &key).unwrap();
        assert_eq!(
            pak.list_dir("fonts"),
            vec!["a.ttf".to_string(), "b.ttc".to_string()]
        );
        assert!(pak.list_dir("no_such_dir").is_empty());
    }

    #[test]
    fn entry_reader_random_seek_matches_full_decrypt() {
        // 512 バイト超のデータでブロック境界を跨ぐシークを何箇所か検証する。
        let mut data = Vec::with_capacity(600);
        let mut seed = 12345u64;
        for _ in 0..600 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            data.push((seed >> 33) as u8);
        }
        // 拡張子を stored 強制にして codec=0 を保証する。
        let entries = vec![("assets/video/demo.avi".to_string(), data.clone())];
        let key = test_key();
        let (bytes, _) = build(entries, &key);
        let pak = Pak::open(bytes, &key).unwrap();

        let full = pak.read("assets/video/demo.avi").unwrap();
        assert_eq!(full, data);

        let mut r = pak.entry_reader("assets/video/demo.avi").unwrap();
        for &(offset, len) in &[
            (0usize, 10usize),
            (63, 5),
            (64, 8),
            (100, 200),
            (17, 1),
            (595, 5),
        ] {
            r.seek(SeekFrom::Start(offset as u64)).unwrap();
            let mut buf = vec![0u8; len];
            r.read_exact(&mut buf).unwrap();
            assert_eq!(buf, data[offset..offset + len], "offset={offset} len={len}");
        }
    }

    #[test]
    fn entry_reader_rejects_deflated_entries() {
        let text = "compressible ".repeat(100);
        let entries = vec![("x.ks".to_string(), text.into_bytes())];
        let key = test_key();
        let (bytes, report) = build(entries, &key);
        assert_eq!(report.codec_deflate_count, 1);
        let pak = Pak::open(bytes, &key).unwrap();
        assert!(pak.entry_reader("x.ks").is_err());
    }

    #[test]
    fn key_slot_unpatched_is_none() {
        assert!(key_slot_key().is_none());
    }

    #[test]
    fn hex_key_parsing() {
        let hex = "42".repeat(32);
        let key = parse_hex_key(&hex).unwrap();
        assert_eq!(key, [0x42u8; 32]);

        assert!(parse_hex_key("too_short").is_err());
        assert!(parse_hex_key(&"zz".repeat(32)).is_err());
    }

    #[test]
    fn dev_key_is_32_bytes_and_stable() {
        assert_eq!(dev_key().len(), 32);
        assert_eq!(dev_key(), DEV_KEY);
    }

    // -----------------------------------------------------------------
    // W9: build_with_special_codec / codec=2 (wav-flac) hook
    // -----------------------------------------------------------------

    #[test]
    fn special_codec_hook_is_used_when_it_returns_some() {
        let plain = b"this is the ORIGINAL logical content".to_vec();
        let precoded = b"XX".to_vec(); // hook が返す「既にエンコード済み」の中身（内容は任意）
        let entries = vec![("fake.wav".to_string(), plain.clone())];
        let key = test_key();
        let (bytes, report) = build_with_special_codec(entries, &key, |path, _data| {
            assert_eq!(path, "fake.wav");
            Some((CODEC_WAV_FLAC, precoded.clone()))
        });
        assert_eq!(report.flac_converted_count, 1);
        assert_eq!(
            report.flac_bytes_saved,
            (plain.len() - precoded.len()) as u64
        );
        assert_eq!(report.codec_stored_count, 0);
        assert_eq!(report.codec_deflate_count, 0);

        let pak = Pak::open(bytes, &key).unwrap();
        assert_eq!(pak.codec_of("fake.wav"), Some(CODEC_WAV_FLAC));
    }

    #[test]
    fn special_codec_hook_falls_back_to_auto_selection_when_none() {
        let text = "same text repeated ".repeat(200);
        let entries = vec![("scenario/x.ks".to_string(), text.clone().into_bytes())];
        let key = test_key();
        let (bytes, report) = build_with_special_codec(entries, &key, |_, _| None);
        assert_eq!(report.codec_deflate_count, 1);
        assert_eq!(report.flac_converted_count, 0);
        let pak = Pak::open(bytes, &key).unwrap();
        assert_eq!(pak.read("scenario/x.ks").unwrap(), text.into_bytes());
    }

    #[test]
    fn build_matches_build_with_special_codec_none_hook() {
        // build() は build_with_special_codec(..., |_, _| None) の薄いラッパであることの回帰確認。
        let entries = vec![("a.txt".to_string(), b"hello world".to_vec())];
        let key = test_key();
        let (bytes_a, report_a) = build(entries.clone(), &key);
        let (bytes_b, report_b) = build_with_special_codec(entries, &key, |_, _| None);
        assert_eq!(bytes_a, bytes_b);
        assert_eq!(report_a.codec_stored_count, report_b.codec_stored_count);
        assert_eq!(report_a.codec_deflate_count, report_b.codec_deflate_count);
    }

    #[test]
    fn read_reconstructs_wav_flac_entry_via_real_codec() {
        // 実際の flac_subset デコーダを経由して、hook が返した wav-flac blob が
        // 正しく読み戻せることを確認する（decode_wav 自体の詳細は codec/flac_subset.rs の
        // 単体テスト、encode 側との実際の round-trip は noiria-tools 側の統合テスト参照）。
        // ここでは pak.rs 側が「codec=2 なら flac_subset::decode_wav を呼ぶ」配線だけを検証する。
        let mut blob = Vec::new();
        blob.extend_from_slice(b"NFC1");
        blob.extend_from_slice(&1u16.to_le_bytes()); // channels=1
        blob.extend_from_slice(&0u32.to_le_bytes()); // num_frames=0
        blob.extend_from_slice(&4u32.to_le_bytes()); // prefix_len
        blob.extend_from_slice(b"PFX!");
        blob.extend_from_slice(&4u32.to_le_bytes()); // suffix_len
        blob.extend_from_slice(b"SFX!");
        // num_frames=0 なのでビットストリーム本体は不要。

        let expected_plain = b"PFX!SFX!".to_vec();
        let entries = vec![("bgm/x.wav".to_string(), expected_plain.clone())];
        let key = test_key();
        let (bytes, _) =
            build_with_special_codec(entries, &key, |_, _| Some((CODEC_WAV_FLAC, blob.clone())));
        let pak = Pak::open(bytes, &key).unwrap();
        assert_eq!(pak.read("bgm/x.wav").unwrap(), expected_plain);
    }
    // -----------------------------------------------------------------
    // R1: 外部入力（壊れた/細工された pak・鍵）の堅牢化
    // -----------------------------------------------------------------

    fn index_plain_of(bytes: &[u8], key: &[u8; 32]) -> Vec<u8> {
        let ofs = u64::from_le_bytes(bytes[16..24].try_into().unwrap()) as usize;
        let len = u64::from_le_bytes(bytes[24..32].try_into().unwrap()) as usize;
        let mut c = bytes[ofs..ofs + len].to_vec();
        crate::crypto::apply_keystream(key, &entry_nonce(INDEX_ENTRY_ID), 0, &mut c);
        crate::codec::inflate::inflate(&c).unwrap()
    }

    /// 平文インデックスを書き換えて再圧縮・再暗号化し、ヘッダの長さだけ直す
    /// （予約領域＝インデックス CRC はそのまま残す）。
    fn with_rewritten_index(bytes: &[u8], key: &[u8; 32], f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut plain = index_plain_of(bytes, key);
        f(&mut plain);
        let mut c = crate::codec::deflate::deflate_compress(&plain);
        crate::crypto::apply_keystream(key, &entry_nonce(INDEX_ENTRY_ID), 0, &mut c);
        let ofs = u64::from_le_bytes(bytes[16..24].try_into().unwrap()) as usize;
        let mut out = bytes[..ofs].to_vec();
        out.extend_from_slice(&c);
        out[24..32].copy_from_slice(&(c.len() as u64).to_le_bytes());
        out[32..40].copy_from_slice(&(plain.len() as u64).to_le_bytes());
        out
    }

    /// 旧実装（インデックス CRC 追加前・`data_section` を別 Vec に作る方式）のバイト列を再現する
    /// 参照実装。新しい書き込み側が「ヘッダの予約領域以外は 1 バイトも変えない」ことの検証用。
    fn legacy_build(entries: Vec<(String, Vec<u8>)>, key: &[u8; 32]) -> Vec<u8> {
        let mut data_section = Vec::new();
        let mut index_entries: Vec<(String, IndexEntry)> = Vec::new();
        for (entry_id, (path, plain)) in entries.into_iter().enumerate() {
            let entry_id = entry_id as u64;
            let crc = crate::codec::png::crc32(&plain);
            let (codec, stored_bytes) = compress_for_entry(&path, &plain);
            let mut cipher = stored_bytes;
            crate::crypto::apply_keystream(key, &entry_nonce(entry_id), 0, &mut cipher);
            let offset = HEADER_LEN as u64 + data_section.len() as u64;
            let stored_len = cipher.len() as u64;
            data_section.extend_from_slice(&cipher);
            index_entries.push((
                path,
                IndexEntry {
                    offset,
                    stored_len,
                    plain_len: plain.len() as u64,
                    codec,
                    crc32: crc,
                    entry_id,
                },
            ));
        }
        let mut index_plain = Vec::new();
        index_plain.extend_from_slice(&(index_entries.len() as u32).to_le_bytes());
        for (path, e) in &index_entries {
            write_index_entry(&mut index_plain, path, e);
        }
        let mut index_cipher = crate::codec::deflate::deflate_compress(&index_plain);
        crate::crypto::apply_keystream(key, &entry_nonce(INDEX_ENTRY_ID), 0, &mut index_cipher);
        let index_ofs = HEADER_LEN as u64 + data_section.len() as u64;
        let mut out = Vec::new();
        let mut h = [0u8; HEADER_LEN];
        h[0..4].copy_from_slice(MAGIC);
        h[4..6].copy_from_slice(&VERSION.to_le_bytes());
        h[8..16].copy_from_slice(&key_check_value(key));
        h[16..24].copy_from_slice(&index_ofs.to_le_bytes());
        h[24..32].copy_from_slice(&(index_cipher.len() as u64).to_le_bytes());
        h[32..40].copy_from_slice(&(index_plain.len() as u64).to_le_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(&data_section);
        out.extend_from_slice(&index_cipher);
        out
    }

    fn sample_entries() -> Vec<(String, Vec<u8>)> {
        vec![
            ("game.json".to_string(), br#"{"title":"t"}"#.to_vec()),
            (
                "scenario/x.ks".to_string(),
                "same text ".repeat(300).into_bytes(),
            ),
            (
                "assets/bg/room.png".to_string(),
                (0..=255u8).cycle().take(5000).collect(),
            ),
        ]
    }

    #[test]
    fn index_corruption_is_detected_by_header_crc() {
        let key = test_key();
        let (bytes, _) = build(sample_entries(), &key);
        // パス末尾の 1 文字を変える（UTF-8 のまま・長さ不変なので、展開後サイズ検査では見つからない）。
        let broken = with_rewritten_index(&bytes, &key, |p| {
            let at = 4 + 2 + "game.json".len() - 1;
            p[at] = b'X';
        });
        let err = Pak::open(broken, &key)
            .err()
            .expect("索引の改変を検出できていない");
        assert!(format!("{err:#}").contains("CRC"), "{err:#}");
    }

    #[test]
    fn index_crc_is_written_and_zero_means_legacy() {
        let key = test_key();
        let (bytes, _) = build(sample_entries(), &key);
        let crc = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        assert_eq!(crc, crate::codec::png::crc32(&index_plain_of(&bytes, &key)));
        assert_eq!(&bytes[44..48], &[0u8; 4]);
        // 予約領域が 0 の旧形式は、索引が改変されていても検査せず開く（従来どおり）。
        let mut legacy = with_rewritten_index(&bytes, &key, |p| {
            let at = 4 + 2 + "game.json".len() - 1;
            p[at] = b'X';
        });
        legacy[40..48].fill(0);
        let pak = Pak::open(legacy, &key).unwrap();
        assert!(pak.exists("game.jsoX"));
    }

    #[test]
    fn written_pak_is_identical_to_legacy_layout_except_reserved_header() {
        let key = test_key();
        let legacy = legacy_build(sample_entries(), &key);
        let (new, _) = build(sample_entries(), &key);
        assert_eq!(new.len(), legacy.len());
        let mut new_masked = new.clone();
        new_masked[40..48].fill(0);
        assert_eq!(
            new_masked, legacy,
            "ヘッダ予約領域以外のバイト列が変わっている"
        );
        // 旧形式の pak を新リーダーで読める。
        let pak = Pak::open(legacy, &key).unwrap();
        for (path, plain) in sample_entries() {
            assert_eq!(pak.read(&path).unwrap(), plain, "{path}");
        }
        let pak = Pak::open(new, &key).unwrap();
        for (path, plain) in sample_entries() {
            assert_eq!(pak.read(&path).unwrap(), plain, "{path}");
        }
    }

    #[test]
    fn build_peak_memory_is_reported() {
        // 参考値の取得用（回帰のしきい値ではなく、ピークが pak の何倍かを出力する）。
        let key = test_key();
        let entries: Vec<(String, Vec<u8>)> = (0..8)
            .map(|i| {
                let mut seed = i as u64 + 1;
                let data: Vec<u8> = (0..1_000_000)
                    .map(|_| {
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                        (seed >> 33) as u8
                    })
                    .collect();
                (format!("assets/bg/{i}.png"), data)
            })
            .collect();
        let (out, peak) = crate::test_support::measure_peak(|| build(entries, &key).0);
        eprintln!(
            "pak build: out={} peak_extra={} ratio={:.2}",
            out.len(),
            peak,
            peak as f64 / out.len() as f64
        );
    }

    #[test]
    fn hex_key_rejects_multibyte_and_signed_input_without_panicking() {
        // 64 バイトだがマルチバイト文字を含む（バイト境界スライスだと char boundary で panic）。
        let tricky = format!("a{}a", "é".repeat(31));
        assert_eq!(tricky.len(), 64);
        assert!(parse_hex_key(&tricky).is_err());
        // from_str_radix は "+f" を受理するが、16 進数字ではない。
        assert!(parse_hex_key(&"+f".repeat(32)).is_err());
        assert!(parse_hex_key(&"-1".repeat(32)).is_err());
        assert!(parse_hex_key(&format!(" {}", "4".repeat(63))).is_err());
        // 大文字小文字どちらの 16 進数字も受理する。
        assert_eq!(parse_hex_key(&"aB".repeat(32)).unwrap(), [0xABu8; 32]);
    }

    #[test]
    fn entry_reader_with_plain_len_larger_than_stored_len_does_not_panic() {
        let key = test_key();
        let (bytes, _) = build(vec![("a.png".to_string(), vec![1u8; 4])], &key);
        let mut pak = Pak::open(bytes, &key).unwrap();
        pak.entries.get_mut("a.png").unwrap().plain_len = 1000; // stored_len は 4 のまま
        match pak.entry_reader("a.png") {
            Err(_) => {}
            Ok(mut r) => {
                let mut buf = vec![0u8; 100];
                let mut total = 0;
                while let Ok(n) = r.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    total += n;
                }
                assert!(total <= 4, "stored_len を超えて読めてしまう: {total}");
            }
        }
    }

    #[test]
    fn deflate_entry_is_not_inflated_beyond_its_declared_plain_len() {
        let key = test_key();
        let (bytes, _) = build(vec![("zeros.bin".to_string(), vec![0u8; 16 << 20])], &key);
        // 索引の plain_len（path_len(2)+path+offset(8)+stored_len(8) の後ろ）だけ 100 に偽装する。
        let mut forged = with_rewritten_index(&bytes, &key, |p| {
            let at = 4 + 2 + "zeros.bin".len() + 16;
            p[at..at + 8].copy_from_slice(&100u64.to_le_bytes());
        });
        // 索引の CRC は改変後の内容に合わせ直す（CRC 検査を通した上で、展開上限だけを試す）。
        let crc = crate::codec::png::crc32(&index_plain_of(&forged, &key));
        forged[40..44].copy_from_slice(&crc.to_le_bytes());
        let pak = Pak::open(forged, &key).unwrap();
        let r = crate::test_support::with_alloc_limit(1 << 20, || pak.read("zeros.bin"));
        assert!(r.is_err());
    }

    #[test]
    fn index_entry_count_does_not_preallocate_from_the_u32_field() {
        // 件数 0xFFFFFFFF を名乗る 4 バイトだけの索引。
        let r = crate::test_support::with_alloc_limit(64 << 20, || {
            parse_index(&0xFFFF_FFFFu32.to_le_bytes())
        });
        assert!(r.is_err());
    }
    // --- 破壊 fuzz（R1） ---

    /// DEFLATE / stored / wav-flac(codec=2) の 3 種を含む小さな pak。
    fn fuzz_pak() -> (Vec<u8>, [u8; 32]) {
        let mut blob = Vec::new();
        blob.extend_from_slice(b"NFC1");
        blob.extend_from_slice(&1u16.to_le_bytes());
        blob.extend_from_slice(&5u32.to_le_bytes());
        blob.extend_from_slice(&4u32.to_le_bytes());
        blob.extend_from_slice(b"PFX!");
        blob.extend_from_slice(&4u32.to_le_bytes());
        blob.extend_from_slice(b"SFX!");
        blob.extend_from_slice(&[0x04, 0x8D, 0x00]); // CONSTANT 0x1234
        let mut wav_plain = b"PFX!".to_vec();
        for _ in 0..5 {
            wav_plain.extend_from_slice(&[0x34, 0x12]);
        }
        wav_plain.extend_from_slice(b"SFX!");
        let mut entries = sample_entries();
        entries.push(("bgm/x.wav".to_string(), wav_plain));
        let key = test_key();
        let (bytes, _) = build_with_special_codec(entries, &key, |path, _| {
            path.ends_with(".wav")
                .then(|| (CODEC_WAV_FLAC, blob.clone()))
        });
        (bytes, key)
    }

    fn read_everything(pak: &Pak) {
        let names: Vec<String> = pak.entries.keys().cloned().collect();
        for name in names {
            let _ = pak.read(&name);
            if let Ok(mut r) = pak.entry_reader(&name) {
                let mut buf = [0u8; 64];
                let mut guard = 0;
                while matches!(r.read(&mut buf), Ok(n) if n > 0) && guard < 64 {
                    guard += 1;
                }
                let _ = r.seek(SeekFrom::End(-3));
                let _ = r.read(&mut buf);
            }
        }
        let _ = pak.list_dir("");
        let _ = pak.list_dir("assets");
    }

    #[test]
    fn fuzz_corrupted_pak_file_never_panics_or_overallocates() {
        let (base, key) = fuzz_pak();
        let pak = Pak::open(base.clone(), &key).unwrap();
        for name in pak.entries.keys() {
            assert!(pak.read(name).is_ok(), "基準の pak の {name} が読める");
        }
        crate::test_support::fuzz("pak", 0x9999, 1500, &base, |d| {
            if let Ok(pak) = Pak::open(d.to_vec(), &key) {
                read_everything(&pak);
            }
        });
    }

    #[test]
    fn fuzz_corrupted_pak_index_never_panics_or_overallocates() {
        let (base, key) = fuzz_pak();
        let plain = index_plain_of(&base, &key);
        // 平文インデックスを直接パースする経路。
        crate::test_support::fuzz("pak index (plain)", 0xAAAA, 3000, &plain, |d| {
            let _ = parse_index(d);
        });
        // 壊した索引を（長さ・CRC を整合させて）封じ直した pak を開く経路。open の検査を通り抜けた後の
        // read / entry_reader が、矛盾した offset・長さ・codec で panic しないこと。
        crate::test_support::fuzz_with(
            "pak index (resealed)",
            0xBBBB,
            1500,
            |rng| {
                let mutated = crate::test_support::mutate(&plain, rng);
                let mut out = with_rewritten_index(&base, &key, |p| *p = mutated);
                let crc = crate::codec::png::crc32(&index_plain_of(&out, &key));
                out[40..44].copy_from_slice(&crc.to_le_bytes());
                out
            },
            |d| {
                if let Ok(pak) = Pak::open(d.to_vec(), &key) {
                    read_everything(&pak);
                }
            },
        );
    }

    struct TempPakFile {
        path: std::path::PathBuf,
    }

    impl TempPakFile {
        fn write(tag: &str, bytes: &[u8]) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "noiria_pak_test_{tag}_{}_{n}.pak",
                std::process::id()
            ));
            std::fs::write(&path, bytes).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempPakFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn pak_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Pak>();
    }

    #[test]
    fn open_file_roundtrip_stored_deflate_and_wav_flac() {
        let mut wav_blob = Vec::new();
        wav_blob.extend_from_slice(b"NFC1");
        wav_blob.extend_from_slice(&1u16.to_le_bytes());
        wav_blob.extend_from_slice(&4u32.to_le_bytes());
        wav_blob.extend_from_slice(&4u32.to_le_bytes());
        wav_blob.extend_from_slice(b"PFX!");
        wav_blob.extend_from_slice(&4u32.to_le_bytes());
        wav_blob.extend_from_slice(b"SFX!");
        wav_blob.extend_from_slice(&[0x04, 0x8D, 0x00]); // CONSTANT 0x1234

        let mut wav_plain = b"PFX!".to_vec();
        for _ in 0..4 {
            wav_plain.extend_from_slice(&[0x34, 0x12]);
        }
        wav_plain.extend_from_slice(b"SFX!");

        let stored_plain: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let deflate_plain = "repeated scenario line\n".repeat(300).into_bytes();

        let entries = vec![
            ("assets/bg/room.png".to_string(), stored_plain.clone()),
            ("scenario/first.ks".to_string(), deflate_plain.clone()),
            ("bgm/theme.wav".to_string(), wav_plain.clone()),
            ("empty.png".to_string(), Vec::new()),
        ];
        let key = test_key();
        let (bytes, report) = build_with_special_codec(entries.clone(), &key, |path, _| {
            path.ends_with(".wav")
                .then(|| (CODEC_WAV_FLAC, wav_blob.clone()))
        });
        assert_eq!(report.codec_stored_count, 2);
        assert_eq!(report.codec_deflate_count, 1);
        assert_eq!(report.flac_converted_count, 1);

        let tmp = TempPakFile::write("roundtrip", &bytes);
        let pak = Pak::open_file(tmp.path(), &key).unwrap();

        assert_eq!(pak.codec_of("assets/bg/room.png"), Some(CODEC_STORED));
        assert_eq!(pak.codec_of("scenario/first.ks"), Some(CODEC_DEFLATE));
        assert_eq!(pak.codec_of("bgm/theme.wav"), Some(CODEC_WAV_FLAC));
        assert_eq!(pak.codec_of("empty.png"), Some(CODEC_STORED));
        assert!(!pak.exists("missing.txt"));
        assert_eq!(pak.list_dir("bgm"), vec!["theme.wav".to_string()]);

        for (path, expected) in &entries {
            assert!(pak.exists(path));
            assert_eq!(&pak.read(path).unwrap(), expected, "path={path}");
        }
    }

    #[test]
    fn open_file_does_not_load_payload_into_ram() {
        let mut seed = 0x1234_5678_9ABC_DEF0u64;
        let big: Vec<u8> = (0..(4 << 20))
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                (seed >> 33) as u8
            })
            .collect();
        let small = br#"{"title":"small"}"#.to_vec();
        let key = test_key();
        let (bytes, _) = build(
            vec![
                ("assets/big.png".to_string(), big.clone()),
                ("game.json".to_string(), small.clone()),
            ],
            &key,
        );
        let tmp = TempPakFile::write("no_ram_resident", &bytes);
        drop(bytes);

        let (pak, peak) = crate::test_support::measure_peak(|| {
            crate::test_support::with_alloc_limit(512 << 10, || {
                Pak::open_file(tmp.path(), &key).unwrap()
            })
        });
        assert!(
            peak < (512 << 10),
            "open_file がアーカイブ本体相当のメモリを確保している: peak={peak}"
        );

        let got_small =
            crate::test_support::with_alloc_limit(512 << 10, || pak.read("game.json").unwrap());
        assert_eq!(got_small, small);

        // PakEntryReader もエントリ全体を確保せずチャンク単位で読める。
        crate::test_support::with_alloc_limit(64 << 10, || {
            let mut r = pak.entry_reader("assets/big.png").unwrap();
            let mut chunk = [0u8; 4096];
            r.seek(SeekFrom::Start(3 << 20)).unwrap();
            r.read_exact(&mut chunk).unwrap();
            assert_eq!(&chunk[..], &big[(3 << 20)..(3 << 20) + 4096]);
        });
    }

    #[test]
    fn open_file_entry_reader_chunked_and_random_seek() {
        let mut data = Vec::with_capacity(1500);
        let mut seed = 98765u64;
        for _ in 0..1500 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            data.push((seed >> 33) as u8);
        }
        let key = test_key();
        let (bytes, _) = build(
            vec![
                ("assets/video/demo.avi".to_string(), data.clone()),
                (
                    "scenario/c.ks".to_string(),
                    "compressible ".repeat(200).into_bytes(),
                ),
            ],
            &key,
        );
        let tmp = TempPakFile::write("entry_reader", &bytes);
        let pak = Pak::open_file(tmp.path(), &key).unwrap();

        assert!(pak.entry_reader("scenario/c.ks").is_err());

        // 様々なチャンク幅（64 バイト ChaCha20 ブロック境界を跨ぐ）で先頭から末尾まで通読。
        let mut r = pak.entry_reader("assets/video/demo.avi").unwrap();
        let mut streamed = Vec::new();
        for &chunk_size in &[1usize, 13, 63, 64, 65, 127, 256] {
            let mut buf = vec![0u8; chunk_size];
            let n = r.read(&mut buf).unwrap();
            streamed.extend_from_slice(&buf[..n]);
        }
        let mut tail = Vec::new();
        r.read_to_end(&mut tail).unwrap();
        streamed.extend_from_slice(&tail);
        assert_eq!(streamed, data);

        // Start / Current / End のシークと負オフセット・EOF 超え。
        r.seek(SeekFrom::Start(63)).unwrap();
        let mut buf = [0u8; 10];
        r.read_exact(&mut buf).unwrap();
        assert_eq!(&buf[..], &data[63..73]);

        r.seek(SeekFrom::Current(55)).unwrap();
        r.read_exact(&mut buf).unwrap();
        assert_eq!(&buf[..], &data[128..138]);

        r.seek(SeekFrom::End(-10)).unwrap();
        r.read_exact(&mut buf).unwrap();
        assert_eq!(&buf[..], &data[data.len() - 10..]);

        assert_eq!(r.read(&mut buf).unwrap(), 0);
        assert!(r.seek(SeekFrom::Current(-(data.len() as i64) - 1)).is_err());
        assert!(r.seek(SeekFrom::End(-(data.len() as i64) - 1)).is_err());
        r.seek(SeekFrom::Start(data.len() as u64 + 100)).unwrap();
        assert_eq!(r.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn open_file_rejects_wrong_key_corrupted_truncated_and_out_of_range() {
        let key = test_key();
        let (bytes, _) = build(sample_entries(), &key);

        // 1. 鍵違い
        let tmp = TempPakFile::write("wrong_key", &bytes);
        let err = Pak::open_file(tmp.path(), &[0x99u8; 32]).err().unwrap();
        assert!(format!("{err:#}").contains("鍵が違います"), "{err:#}");

        // 2. 不正マジック
        let mut bad_magic = bytes.clone();
        bad_magic[0..4].copy_from_slice(b"NOPE");
        let tmp = TempPakFile::write("bad_magic", &bad_magic);
        assert!(Pak::open_file(tmp.path(), &key).is_err());

        // 3. 未対応バージョン
        let mut bad_ver = bytes.clone();
        bad_ver[4..6].copy_from_slice(&99u16.to_le_bytes());
        let tmp = TempPakFile::write("bad_ver", &bad_ver);
        assert!(Pak::open_file(tmp.path(), &key).is_err());

        // 4. ヘッダ未満に切り詰められたファイル
        let tmp = TempPakFile::write("trunc_hdr", &bytes[..20]);
        assert!(Pak::open_file(tmp.path(), &key).is_err());

        // 5. インデックス途中で切り詰められたファイル
        let tmp = TempPakFile::write("trunc_idx", &bytes[..bytes.len() - 4]);
        assert!(Pak::open_file(tmp.path(), &key).is_err());

        // 6. インデックス CRC 不一致
        let broken_crc = with_rewritten_index(&bytes, &key, |p| {
            let at = 4 + 2 + "game.json".len() - 1;
            p[at] = b'X';
        });
        let tmp = TempPakFile::write("bad_crc", &broken_crc);
        assert!(Pak::open_file(tmp.path(), &key).is_err());

        // 7. 範囲外オフセットを指すエントリ（CRC は合わせて open_file を通過させた上で read/entry_reader が拒否）
        let mut bad_ofs = with_rewritten_index(&bytes, &key, |p| {
            // 先頭エントリ (game.json) の offset は 4 + 2 + path_len バイト目。
            let ofs_at = 4 + 2 + "game.json".len();
            p[ofs_at..ofs_at + 8].copy_from_slice(&999_999u64.to_le_bytes());
        });
        let crc = crate::codec::png::crc32(&index_plain_of(&bad_ofs, &key));
        bad_ofs[40..44].copy_from_slice(&crc.to_le_bytes());
        let tmp = TempPakFile::write("bad_ofs", &bad_ofs);
        let pak = Pak::open_file(tmp.path(), &key).unwrap();
        assert!(pak.read("game.json").is_err());
        assert!(pak.entry_reader("game.json").is_err());
        drop(pak);

        // 8. open_file 後にファイルが外部から切り詰められても panic せず Err / EOF を返す。
        let tmp = TempPakFile::write("trunc_after_open", &bytes);
        let pak = Pak::open_file(tmp.path(), &key).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(tmp.path())
            .unwrap()
            .set_len(HEADER_LEN as u64)
            .unwrap();
        assert!(pak.read("assets/bg/room.png").is_err());
        let mut r = pak.entry_reader("assets/bg/room.png").unwrap();
        let mut buf = [0u8; 32];
        assert_eq!(r.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn open_file_concurrent_reads_across_threads() {
        use std::sync::Arc;

        let mut wav_blob = Vec::new();
        wav_blob.extend_from_slice(b"NFC1");
        wav_blob.extend_from_slice(&1u16.to_le_bytes());
        wav_blob.extend_from_slice(&4u32.to_le_bytes());
        wav_blob.extend_from_slice(&4u32.to_le_bytes());
        wav_blob.extend_from_slice(b"PFX!");
        wav_blob.extend_from_slice(&4u32.to_le_bytes());
        wav_blob.extend_from_slice(b"SFX!");
        wav_blob.extend_from_slice(&[0x04, 0x8D, 0x00]);
        let mut wav_plain = b"PFX!".to_vec();
        for _ in 0..4 {
            wav_plain.extend_from_slice(&[0x34, 0x12]);
        }
        wav_plain.extend_from_slice(b"SFX!");

        let stored_a: Vec<u8> = (0..8192u32)
            .map(|i| (i.wrapping_mul(31) >> 3) as u8)
            .collect();
        let stored_b: Vec<u8> = (0..6000u32)
            .map(|i| (i.wrapping_mul(97) >> 2) as u8)
            .collect();
        let deflated: Vec<u8> = "concurrent deflate payload ".repeat(400).into_bytes();

        let key = test_key();
        let (bytes, _) = build_with_special_codec(
            vec![
                ("a.png".to_string(), stored_a.clone()),
                ("b.png".to_string(), stored_b.clone()),
                ("c.ks".to_string(), deflated.clone()),
                ("d.wav".to_string(), wav_plain.clone()),
            ],
            &key,
            |path, _| {
                path.ends_with(".wav")
                    .then(|| (CODEC_WAV_FLAC, wav_blob.clone()))
            },
        );
        let tmp = TempPakFile::write("concurrent", &bytes);
        let pak = Arc::new(Pak::open_file(tmp.path(), &key).unwrap());
        let expected = Arc::new((stored_a, stored_b, deflated, wav_plain));

        let mut handles = Vec::new();
        for tid in 0..8usize {
            let pak = Arc::clone(&pak);
            let expected = Arc::clone(&expected);
            handles.push(std::thread::spawn(move || {
                for iter in 0..25usize {
                    match (tid + iter) % 4 {
                        0 => assert_eq!(pak.read("a.png").unwrap(), expected.0),
                        1 => assert_eq!(pak.read("b.png").unwrap(), expected.1),
                        2 => assert_eq!(pak.read("c.ks").unwrap(), expected.2),
                        _ => assert_eq!(pak.read("d.wav").unwrap(), expected.3),
                    }
                    let mut r = pak.entry_reader("a.png").unwrap();
                    let ofs = ((tid * 337 + iter * 73) % (expected.0.len() - 128)) as u64;
                    r.seek(SeekFrom::Start(ofs)).unwrap();
                    let mut buf = [0u8; 128];
                    r.read_exact(&mut buf).unwrap();
                    assert_eq!(&buf[..], &expected.0[ofs as usize..ofs as usize + 128]);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }
}
