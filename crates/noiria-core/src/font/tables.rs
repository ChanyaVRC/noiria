//! 固定レイアウトの単純なテーブル群: head / maxp / hhea / hmtx / OS/2 / name。
//! glyf 系（可変長・入れ子構造）は `glyf.rs`、cmap は `cmap.rs` に分離。

use anyhow::{ensure, Result};

use super::reader::{u16_at, Reader};

pub(crate) struct Head {
    pub units_per_em: u16,
    pub index_to_loc_format: i16,
}

pub(crate) fn parse_head(data: &[u8]) -> Result<Head> {
    ensure!(data.len() >= 54, "head テーブルが短すぎます");
    let units_per_em = u16_at(data, 18)?;
    ensure!(units_per_em > 0, "unitsPerEm が 0 です");
    let index_to_loc_format = u16_at(data, 50)? as i16;
    ensure!(
        index_to_loc_format == 0 || index_to_loc_format == 1,
        "indexToLocFormat が不正です: {index_to_loc_format}"
    );
    Ok(Head {
        units_per_em,
        index_to_loc_format,
    })
}

pub(crate) fn parse_maxp_num_glyphs(data: &[u8]) -> Result<u16> {
    ensure!(data.len() >= 6, "maxp テーブルが短すぎます");
    u16_at(data, 4)
}

pub(crate) struct Hhea {
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    pub number_of_h_metrics: u16,
}

pub(crate) fn parse_hhea(data: &[u8]) -> Result<Hhea> {
    ensure!(data.len() >= 36, "hhea テーブルが短すぎます");
    Ok(Hhea {
        ascender: u16_at(data, 4)? as i16,
        descender: u16_at(data, 6)? as i16,
        line_gap: u16_at(data, 8)? as i16,
        number_of_h_metrics: u16_at(data, 34)?,
    })
}

/// hmtx: 前方 `number_of_h_metrics` 個は (advanceWidth: u16, lsb: i16) の 4 バイト、
/// 残りは lsb のみの 2 バイト（advance は最後の値を引き継ぐ）。
/// ここでは advance だけ使うので advanceWidth の配列として保持する。
pub(crate) fn parse_hmtx_advances(data: &[u8], number_of_h_metrics: u16) -> Result<Vec<u16>> {
    let n = number_of_h_metrics as usize;
    let mut r = Reader::new(data);
    let mut advances = Vec::with_capacity(n);
    for _ in 0..n {
        let advance = r.u16()?;
        let _lsb = r.i16()?;
        advances.push(advance);
    }
    Ok(advances)
}

/// OS/2: sTypoAscender 系と usWinAscent 系の両方を保持し、
/// 上位（Font）側で fsSelection の USE_TYPO_METRICS ビットに応じて選択する。
pub(crate) struct Os2 {
    pub fs_selection: u16,
    pub typo_ascender: i16,
    pub typo_descender: i16,
    pub typo_line_gap: i16,
}

const USE_TYPO_METRICS: u16 = 0x0080;

impl Os2 {
    pub fn use_typo_metrics(&self) -> bool {
        self.fs_selection & USE_TYPO_METRICS != 0
    }
}

/// OS/2 はバージョンによって長さが異なる（version 0 で 78 バイト）。
/// 短い/壊れたテーブルの場合は None を返し、呼び出し側は hhea にフォールバックする。
pub(crate) fn parse_os2(data: &[u8]) -> Option<Os2> {
    if data.len() < 78 {
        return None;
    }
    Some(Os2 {
        fs_selection: u16_at(data, 62).ok()?,
        typo_ascender: u16_at(data, 68).ok()? as i16,
        typo_descender: u16_at(data, 70).ok()? as i16,
        typo_line_gap: u16_at(data, 72).ok()? as i16,
    })
}

const NAME_ID_FAMILY: u16 = 1;
const PLATFORM_UNICODE: u16 = 0;
const PLATFORM_MAC: u16 = 1;
const PLATFORM_WINDOWS: u16 = 3;

/// name テーブル (format 0/1 共通の先頭レイアウト) からファミリー名 (nameID=1) を探す。
/// Windows/Unicode プラットフォームは UTF-16BE、Mac プラットフォームはこの実装では
/// ASCII 相当として素朴にデコードする（Mac Roman の非 ASCII 記号は近似）。
pub(crate) fn parse_family_name(data: &[u8]) -> Option<String> {
    let mut r = Reader::new(data);
    let _format = r.u16().ok()?;
    let count = r.u16().ok()?;
    let storage_offset = r.u16().ok()? as usize;

    struct Candidate {
        platform_id: u16,
        bytes_range: (usize, usize),
    }
    let mut best: Option<Candidate> = None;
    let score_of = |platform_id: u16| -> i32 {
        match platform_id {
            PLATFORM_WINDOWS => 3,
            PLATFORM_UNICODE => 2,
            PLATFORM_MAC => 1,
            _ => 0,
        }
    };
    let mut best_score = -1;

    for _ in 0..count {
        let platform_id = r.u16().ok()?;
        let _encoding_id = r.u16().ok()?;
        let _language_id = r.u16().ok()?;
        let name_id = r.u16().ok()?;
        let length = r.u16().ok()? as usize;
        let offset = r.u16().ok()? as usize;
        if name_id != NAME_ID_FAMILY {
            continue;
        }
        let start = storage_offset.checked_add(offset)?;
        let end = start.checked_add(length)?;
        if end > data.len() {
            continue;
        }
        let s = score_of(platform_id);
        if s > best_score {
            best_score = s;
            best = Some(Candidate {
                platform_id,
                bytes_range: (start, end),
            });
        }
    }

    let best = best?;
    let raw = &data[best.bytes_range.0..best.bytes_range.1];
    if best.platform_id == PLATFORM_WINDOWS || best.platform_id == PLATFORM_UNICODE {
        decode_utf16be(raw)
    } else {
        // Mac Roman 近似（0x80 以上は '?' に潰す）
        Some(
            raw.iter()
                .map(|&b| if b < 0x80 { b as char } else { '?' })
                .collect(),
        )
    }
}

/// name テーブルの nameID=1 (Family) レコードを全件返す（platform_id, language_id, 文字列）。
/// `parse_family_name` はスコア最良の 1 件だけを返すのに対し、こちらは全レコードを
/// 返す。`font::registry` がローカライズ名（例: 日本語の "游ゴシック"）も含めて
/// family 名からフォントを引けるようにするために使う。
pub(crate) fn parse_family_names(data: &[u8]) -> Vec<(u16, u16, String)> {
    let mut out = Vec::new();
    let mut r = Reader::new(data);
    let Ok(_format) = r.u16() else { return out };
    let Ok(count) = r.u16() else { return out };
    let Ok(storage_offset) = r.u16().map(|v| v as usize) else {
        return out;
    };

    for _ in 0..count {
        let Ok(platform_id) = r.u16() else { break };
        let Ok(_encoding_id) = r.u16() else { break };
        let Ok(language_id) = r.u16() else { break };
        let Ok(name_id) = r.u16() else { break };
        let Ok(length) = r.u16().map(|v| v as usize) else {
            break;
        };
        let Ok(offset) = r.u16().map(|v| v as usize) else {
            break;
        };
        if name_id != NAME_ID_FAMILY {
            continue;
        }
        let Some(start) = storage_offset.checked_add(offset) else {
            continue;
        };
        let Some(end) = start.checked_add(length) else {
            continue;
        };
        if end > data.len() {
            continue;
        }
        let raw = &data[start..end];
        let name = if platform_id == PLATFORM_WINDOWS || platform_id == PLATFORM_UNICODE {
            decode_utf16be(raw)
        } else {
            Some(
                raw.iter()
                    .map(|&b| if b < 0x80 { b as char } else { '?' })
                    .collect(),
            )
        };
        if let Some(name) = name {
            out.push((platform_id, language_id, name));
        }
    }
    out
}

fn decode_utf16be(raw: &[u8]) -> Option<String> {
    if !raw.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).ok()
}
