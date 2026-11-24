//! TrueType (sfnt / TTC) パーサ（自作・依存クレートなし）。
//!
//! cosmic-text 置き換えの段階的自作化: F1 パーサ + アウトライン抽出、F2 スキャンライン
//! AA ラスタライザ ([`raster`] モジュール)、F3 `text.rs` への統合（レイアウト・禁則・
//! フォールバック、[`registry`] モジュール）まで完了。`cosmic-text` は依存から削除済み。
//! このモジュールの外（`text.rs`）へは [`Font`] / [`Outline`] / [`Contour`] / [`Segment`] /
//! [`Point`] / [`rasterize`] / [`GlyphMask`] / [`GlyphCache`] / [`size_q6`] /
//! [`registry`] という自前の型・関数だけを公開する。
//!
//! 対応範囲:
//! - コンテナ: sfnt (単体 TTF, 'true' / 0x00010000) と TTC ('ttcf') コレクション
//! - CFF アウトライン ('OTTO') は明示的に非対応（`Err` を返す）
//! - 必須テーブル: head, maxp, cmap (format 4 / 12), loca, glyf, hhea, hmtx, OS/2, name
//! - グリフ: 単純グリフ（線分 + 2 次ベジェへ正規化）と複合グリフ（ネスト・アフィン変換）
//!
//! すべてのテーブル読み取りは境界チェック付きで、壊れたデータに対しても
//! panic せず `Err` を返す（`font::mod` のテストに LCG 破壊による fuzz テストあり）。

mod cmap;
mod glyf;
mod raster;
mod reader;
pub mod registry;
mod sfnt;
mod tables;

pub use glyf::{Contour, Outline, Point, Segment};
pub use raster::{rasterize, size_q6, GlyphCache, GlyphMask};

use std::borrow::Cow;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use anyhow::{ensure, Result};

/// `glyf` テーブル本体の保持形態。システムフォントはファイルハンドルを保持して
/// グリフ単位でオンデマンド読込し、10〜15 MiB の TTC 全体を RAM に常駐させない。
pub(crate) enum GlyfStorage {
    Owned(Box<[u8]>),
    Shared {
        data: Arc<[u8]>,
        offset: u32,
        len: u32,
    },
    File {
        #[cfg(any(unix, windows))]
        file: File,
        #[cfg(not(any(unix, windows)))]
        file: std::sync::Mutex<File>,
        offset: u32,
        len: u32,
    },
}

impl GlyfStorage {
    pub(crate) fn read_range(&self, start: u32, length: u32) -> Result<Cow<'_, [u8]>> {
        match self {
            Self::Owned(buf) => Ok(Cow::Borrowed(sfnt::table_slice(buf, start, length)?)),
            Self::Shared { data, offset, len } => {
                let glyf = sfnt::table_slice(data, *offset, *len)?;
                Ok(Cow::Borrowed(sfnt::table_slice(glyf, start, length)?))
            }
            Self::File { file, offset, len } => {
                let end = start
                    .checked_add(length)
                    .ok_or_else(|| anyhow::anyhow!("テーブル長の計算がオーバーフローしました"))?;
                ensure!(end <= *len, "テーブルがファイル範囲外を指しています");
                let abs = u64::from(*offset) + u64::from(start);
                let mut buf = vec![0u8; length as usize];
                read_file_exact_at(file, abs, &mut buf)?;
                Ok(Cow::Owned(buf))
            }
        }
    }
}

#[cfg(any(unix, windows))]
fn read_file_exact_at(file: &File, mut offset: u64, mut buf: &mut [u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::FileExt;
    #[cfg(windows)]
    use std::os::windows::fs::FileExt;

    while !buf.is_empty() {
        #[cfg(unix)]
        let res = file.read_at(buf, offset);
        #[cfg(windows)]
        let res = file.seek_read(buf, offset);
        match res {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "フォントファイル終端に到達しました",
                ));
            }
            Ok(n) => {
                offset += n as u64;
                buf = &mut buf[n..];
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn read_file_exact_at(
    file: &std::sync::Mutex<File>,
    offset: u64,
    buf: &mut [u8],
) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = file.lock().unwrap_or_else(|e| e.into_inner());
    f.seek(SeekFrom::Start(offset))?;
    f.read_exact(buf)
}

/// 解析済みフォント 1 個（TTC の場合はコレクション中の 1 フォント分）。
pub struct Font {
    glyf: GlyfStorage,
    family_name: Box<str>,
    units_per_em: u16,
    ascent: i16,
    descent: i16,
    line_gap: i16,
    num_glyphs: u16,
    loca: Box<[u32]>,
    hmtx_advances: Box<[u16]>,
    cmap: cmap::CmapSubtable,
}

impl Font {
    /// バイト列からフォントを解析する。`index` は TTC 内のフォント番号
    /// （単体 sfnt ファイルでは 0 のみ有効）。
    pub fn from_bytes(data: Vec<u8>, index: u32) -> Result<Font> {
        let dir = sfnt::parse_table_directory(&data, index)?;
        let (glyf_offset, glyf_length) = dir
            .find(b"glyf")
            .ok_or_else(|| anyhow::anyhow!("必須テーブル 'glyf' がありません"))?;
        let glyf_bytes: Box<[u8]> = sfnt::table_slice(&data, glyf_offset, glyf_length)?.into();
        Self::parse_from_slice(&data, &dir, GlyfStorage::Owned(glyf_bytes))
    }

    /// 共有バイト列（ゲーム同梱フォント等）からコピーなしで構築する。
    pub(crate) fn from_shared_bytes(data: Arc<[u8]>, index: u32) -> Result<Font> {
        let dir = sfnt::parse_table_directory(&data, index)?;
        let (glyf_offset, glyf_length) = dir
            .find(b"glyf")
            .ok_or_else(|| anyhow::anyhow!("必須テーブル 'glyf' がありません"))?;
        sfnt::table_slice(&data, glyf_offset, glyf_length)?;
        Self::parse_from_slice(
            &data,
            &dir,
            GlyfStorage::Shared {
                data: Arc::clone(&data),
                offset: glyf_offset,
                len: glyf_length,
            },
        )
    }

    /// ディスク上のフォントファイルから最小限のテーブルのみ読み込んで構築する。
    /// `glyf` 本体はオンデマンド読込とするため、大規模 TTC でも常駐メモリを抑えられる。
    pub(crate) fn from_file(path: &Path, index: u32) -> Result<Font> {
        let file = File::open(path)?;
        let file_len = file.metadata()?.len();
        #[cfg(any(unix, windows))]
        let file_ref = &file;
        #[cfg(not(any(unix, windows)))]
        let file = std::sync::Mutex::new(file);
        #[cfg(not(any(unix, windows)))]
        let file_ref = &file;

        ensure!(file_len >= 4, "ファイルが短すぎます");
        let mut tag_bytes = [0u8; 4];
        read_file_exact_at(file_ref, 0, &mut tag_bytes)?;
        let sfnt_offset: u64 = if &tag_bytes == b"ttcf" {
            ensure!(file_len >= 12, "ファイルが短すぎます");
            let mut ttc_hdr = [0u8; 8];
            read_file_exact_at(file_ref, 4, &mut ttc_hdr)?;
            let num_fonts = u32::from_be_bytes([ttc_hdr[4], ttc_hdr[5], ttc_hdr[6], ttc_hdr[7]]);
            ensure!(
                index < num_fonts,
                "フォントインデックス {index} が範囲外です（コレクション内 {num_fonts} 個）"
            );
            let off_pos = 12u64 + u64::from(index) * 4;
            ensure!(off_pos + 4 <= file_len, "ファイルが短すぎます");
            let mut off_bytes = [0u8; 4];
            read_file_exact_at(file_ref, off_pos, &mut off_bytes)?;
            u64::from(u32::from_be_bytes(off_bytes))
        } else {
            ensure!(
                index == 0,
                "単体フォントファイルでインデックス {index} は指定できません"
            );
            0
        };

        let mut read_at = |off: u64, buf: &mut [u8]| read_file_exact_at(file_ref, off, buf);
        let mut dir_buf = Vec::new();
        let dir = sfnt::read_sfnt_directory_at(&mut read_at, file_len, sfnt_offset, &mut dir_buf)?;

        let (glyf_offset, glyf_length) = dir
            .find(b"glyf")
            .ok_or_else(|| anyhow::anyhow!("必須テーブル 'glyf' がありません"))?;
        ensure!(
            u64::from(glyf_offset)
                .checked_add(u64::from(glyf_length))
                .is_some_and(|e| e <= file_len),
            "テーブルがファイル範囲外を指しています"
        );

        let mut read_opt = |tag: &[u8; 4]| -> Result<Option<Cow<'_, [u8]>>> {
            let Some((off, len)) = dir.find(tag) else {
                return Ok(None);
            };
            let end = u64::from(off)
                .checked_add(u64::from(len))
                .ok_or_else(|| anyhow::anyhow!("テーブル長の計算がオーバーフローしました"))?;
            ensure!(end <= file_len, "テーブルがファイル範囲外を指しています");
            let mut buf = vec![0u8; len as usize];
            read_at(u64::from(off), &mut buf)?;
            Ok(Some(Cow::Owned(buf)))
        };
        let font = Self::parse_tables(&mut read_opt)?;
        Ok(Font {
            glyf: GlyfStorage::File {
                file,
                offset: glyf_offset,
                len: glyf_length,
            },
            ..font
        })
    }

    fn parse_from_slice(
        data: &[u8],
        dir: &sfnt::TableDirectory,
        glyf: GlyfStorage,
    ) -> Result<Font> {
        let mut get = |tag: &[u8; 4]| -> Result<Option<Cow<'_, [u8]>>> {
            Ok(dir.find_slice(data, tag)?.map(Cow::Borrowed))
        };
        let font = Self::parse_tables(&mut get)?;
        Ok(Font { glyf, ..font })
    }

    fn parse_tables<'a>(
        get: &mut impl FnMut(&[u8; 4]) -> Result<Option<Cow<'a, [u8]>>>,
    ) -> Result<Font> {
        let mut req = |tag: &[u8; 4]| -> Result<Cow<'a, [u8]>> {
            get(tag)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "必須テーブル '{}' がありません",
                    String::from_utf8_lossy(tag)
                )
            })
        };
        let head = tables::parse_head(&req(b"head")?)?;
        let num_glyphs = tables::parse_maxp_num_glyphs(&req(b"maxp")?)?;
        let hhea = tables::parse_hhea(&req(b"hhea")?)?;
        let hmtx_advances = tables::parse_hmtx_advances(&req(b"hmtx")?, hhea.number_of_h_metrics)?;
        let cmap = cmap::parse_cmap(&req(b"cmap")?)?;
        let loca = glyf::parse_loca(&req(b"loca")?, num_glyphs, head.index_to_loc_format == 1)?;
        let os2 = get(b"OS/2")?.as_deref().and_then(tables::parse_os2);
        let family_name = get(b"name")?
            .as_deref()
            .and_then(tables::parse_family_name)
            .unwrap_or_default();
        ensure!(
            loca.len() == num_glyphs as usize + 1,
            "loca テーブルの要素数が numGlyphs と一致しません"
        );
        let (ascent, descent, line_gap) = match &os2 {
            Some(o) if o.use_typo_metrics() => (o.typo_ascender, o.typo_descender, o.typo_line_gap),
            _ => (hhea.ascender, hhea.descender, hhea.line_gap),
        };
        Ok(Font {
            glyf: GlyfStorage::Owned(Box::new([])),
            family_name: family_name.into_boxed_str(),
            units_per_em: head.units_per_em,
            ascent,
            descent,
            line_gap,
            num_glyphs,
            loca: loca.into_boxed_slice(),
            hmtx_advances: hmtx_advances.into_boxed_slice(),
            cmap,
        })
    }

    pub fn family_name(&self) -> &str {
        &self.family_name
    }

    pub fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    pub fn ascent(&self) -> i16 {
        self.ascent
    }

    pub fn descent(&self) -> i16 {
        self.descent
    }

    pub fn line_gap(&self) -> i16 {
        self.line_gap
    }

    pub fn num_glyphs(&self) -> u16 {
        self.num_glyphs
    }

    /// Unicode 文字からグリフ ID を引く（cmap format 4 / 12。BMP 外も対応）。
    pub fn glyph_id(&self, c: char) -> Option<u16> {
        self.cmap.lookup(c)
    }

    /// 水平アドバンス幅（フォント単位）。`numberOfHMetrics` を超えるグリフ ID は
    /// hmtx 仕様どおり最後のアドバンス値を引き継ぐ。
    pub fn advance(&self, glyph: u16) -> u16 {
        match self.hmtx_advances.last() {
            None => 0,
            Some(&last) => self
                .hmtx_advances
                .get(glyph as usize)
                .copied()
                .unwrap_or(last),
        }
    }

    /// グリフのアウトライン（フォント単位座標、閉路群）を取り出す。
    pub fn outline(&self, glyph: u16) -> Result<Outline> {
        glyf::outline_for_glyph(&self.glyf, &self.loca, glyph, self.num_glyphs)
    }
}

// テスト専用: `registry`/`text` の W1 テストが手組みフォントを再利用するため `pub(crate)`。
#[cfg(test)]
pub(crate) mod tests;
