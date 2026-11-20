//! cmap テーブル: 文字コード (Unicode) → グリフ ID。
//! format 4 (BMP, セグメント方式) と format 12 (全域, 連続グループ方式) に対応。
//! それ以外の format のサブテーブルは無視する（未対応）。

use anyhow::{ensure, Result};

use super::reader::Reader;

pub(crate) enum CmapSubtable {
    Format4(Format4Table),
    Format12(Format12Table),
}

impl CmapSubtable {
    pub fn lookup(&self, c: char) -> Option<u16> {
        match self {
            CmapSubtable::Format4(t) => {
                let cp = c as u32;
                if cp > 0xFFFF {
                    None
                } else {
                    t.lookup(cp as u16)
                }
            }
            CmapSubtable::Format12(t) => t.lookup(c as u32),
        }
    }
}

pub(crate) struct Format4Table {
    /// サブテーブル先頭 (format フィールド位置) からファイル末尾までのバイト列。
    /// idRangeOffset のポインタ演算がサブテーブルの `length` を超えて
    /// glyphIdArray を指すような（仕様上あり得る）ケースでも安全に読めるよう、
    /// ファイル末尾までを保持しておく。
    bytes: Vec<u8>,
    seg_count: usize,
    end_code_off: usize,
    start_code_off: usize,
    id_delta_off: usize,
    id_range_offset_off: usize,
}

fn u16_opt(data: &[u8], off: usize) -> Option<u16> {
    let end = off.checked_add(2)?;
    let s = data.get(off..end)?;
    Some(u16::from_be_bytes([s[0], s[1]]))
}

impl Format4Table {
    fn end_code(&self, i: usize) -> Option<u16> {
        u16_opt(&self.bytes, self.end_code_off + i * 2)
    }
    fn start_code(&self, i: usize) -> Option<u16> {
        u16_opt(&self.bytes, self.start_code_off + i * 2)
    }
    fn id_delta(&self, i: usize) -> Option<i16> {
        u16_opt(&self.bytes, self.id_delta_off + i * 2).map(|v| v as i16)
    }
    fn id_range_offset(&self, i: usize) -> Option<u16> {
        u16_opt(&self.bytes, self.id_range_offset_off + i * 2)
    }

    pub fn lookup(&self, cp: u16) -> Option<u16> {
        if self.seg_count == 0 {
            return None;
        }
        // endCode 配列は昇順なので、cp <= endCode[i] となる最小の i を二分探索する。
        let mut lo = 0usize;
        let mut hi = self.seg_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let end = self.end_code(mid)?;
            if end < cp {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo >= self.seg_count {
            return None;
        }
        let i = lo;
        let start = self.start_code(i)?;
        if cp < start {
            return None;
        }
        let id_delta = self.id_delta(i)?;
        let id_range_offset = self.id_range_offset(i)?;

        let raw_glyph: u16 = if id_range_offset == 0 {
            (cp as i32 + id_delta as i32) as u16
        } else {
            let addr = self
                .id_range_offset_off
                .checked_add(i * 2)?
                .checked_add(id_range_offset as usize)?
                .checked_add(2 * (cp - start) as usize)?;
            let g = u16_opt(&self.bytes, addr)?;
            if g == 0 {
                0
            } else {
                (g as i32 + id_delta as i32) as u16
            }
        };
        if raw_glyph == 0 {
            None
        } else {
            Some(raw_glyph)
        }
    }
}

pub(crate) struct Format12Table {
    /// (startCharCode, endCharCode, startGlyphID) を startCharCode 昇順に並べたもの
    groups: Vec<(u32, u32, u32)>,
}

impl Format12Table {
    pub fn lookup(&self, cp: u32) -> Option<u16> {
        let idx = self
            .groups
            .binary_search_by(|g| {
                if cp < g.0 {
                    std::cmp::Ordering::Greater
                } else if cp > g.1 {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()?;
        let (start, _end, start_glyph) = self.groups[idx];
        let g = start_glyph.checked_add(cp - start)?;
        u16::try_from(g).ok().filter(|&g| g != 0)
    }
}

fn parse_format4(data: &[u8], off: usize) -> Result<Format4Table> {
    let mut r = Reader::at(data, off)?;
    let format = r.u16()?;
    ensure!(format == 4, "format4 ではありません");
    let _length = r.u16()?;
    let _language = r.u16()?;
    let seg_count_x2 = r.u16()?;
    let seg_count = (seg_count_x2 / 2) as usize;
    let _search_range = r.u16()?;
    let _entry_selector = r.u16()?;
    let _range_shift = r.u16()?;

    let end_code_off = r.pos();
    r.skip(
        seg_count
            .checked_mul(2)
            .ok_or_else(|| anyhow::anyhow!("segCount overflow"))?,
    )?;
    let _reserved_pad = r.u16()?;
    let start_code_off = r.pos();
    r.skip(seg_count * 2)?;
    let id_delta_off = r.pos();
    r.skip(seg_count * 2)?;
    let id_range_offset_off = r.pos();
    r.skip(seg_count * 2)?;

    Ok(Format4Table {
        bytes: data[off..].to_vec(),
        seg_count,
        end_code_off: end_code_off - off,
        start_code_off: start_code_off - off,
        id_delta_off: id_delta_off - off,
        id_range_offset_off: id_range_offset_off - off,
    })
}

fn parse_format12(data: &[u8], off: usize) -> Result<Format12Table> {
    let mut r = Reader::at(data, off)?;
    let format = r.u16()?;
    ensure!(format == 12, "format12 ではありません");
    let _reserved = r.u16()?;
    let _length = r.u32()?;
    let _language = r.u32()?;
    let num_groups = r.u32()?;
    // 悪意ある/壊れたファイルで巨大な numGroups を指定されても無限に読まないよう、
    // 残りバイト数（1 グループ 12 バイト）でクランプする。
    let max_groups = (r.remaining() / 12) as u32;
    let n = num_groups.min(max_groups) as usize;
    let mut groups = Vec::with_capacity(n);
    for _ in 0..n {
        let start = r.u32()?;
        let end = r.u32()?;
        let start_glyph = r.u32()?;
        groups.push((start, end, start_glyph));
    }
    groups.sort_by_key(|g| g.0);
    Ok(Format12Table { groups })
}

fn score_encoding(platform_id: u16, encoding_id: u16, format: u16) -> i32 {
    match (platform_id, encoding_id, format) {
        (3, 10, 12) => 100,
        (0, 6, 12) | (0, 4, 12) => 90,
        (3, 1, 4) => 80,
        (0, 3, 4) | (0, 2, 4) | (0, 1, 4) | (0, 0, 4) => 70,
        (_, _, 12) => 50, // 非標準な platform/encoding だが全域を引けるので中程度に優先
        (_, _, 4) => 40,
        _ => -1,
    }
}

/// cmap テーブルから最良のサブテーブル（BMP 外も引ける format 12 を優先）を選んで解析する。
pub(crate) fn parse_cmap(data: &[u8]) -> Result<CmapSubtable> {
    let mut r = Reader::new(data);
    let _version = r.u16()?;
    let num_tables = r.u16()?;

    let mut best: Option<(i32, CmapSubtable)> = None;
    for _ in 0..num_tables {
        let platform_id = r.u16()?;
        let encoding_id = r.u16()?;
        let offset = r.u32()? as usize;
        if offset + 2 > data.len() {
            continue;
        }
        let Ok(format) = super::reader::u16_at(data, offset) else {
            continue;
        };
        let score = score_encoding(platform_id, encoding_id, format);
        if score < 0 {
            continue;
        }
        let parsed = match format {
            4 => parse_format4(data, offset).ok().map(CmapSubtable::Format4),
            12 => parse_format12(data, offset)
                .ok()
                .map(CmapSubtable::Format12),
            _ => None,
        };
        if let Some(table) = parsed {
            let better = best.as_ref().map(|(s, _)| score > *s).unwrap_or(true);
            if better {
                best = Some((score, table));
            }
        }
    }

    best.map(|(_, t)| t).ok_or_else(|| {
        anyhow::anyhow!("使用可能な cmap サブテーブル (format 4/12) が見つかりません")
    })
}
