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

use anyhow::{ensure, Result};

/// 解析済みフォント 1 個（TTC の場合はコレクション中の 1 フォント分）。
pub struct Font {
    data: Vec<u8>,
    family_name: String,
    units_per_em: u16,
    ascent: i16,
    descent: i16,
    line_gap: i16,
    num_glyphs: u16,
    loca: Vec<u32>,
    glyf_offset: u32,
    glyf_length: u32,
    hmtx_advances: Vec<u16>,
    cmap: cmap::CmapSubtable,
}

impl Font {
    /// バイト列からフォントを解析する。`index` は TTC 内のフォント番号
    /// （単体 sfnt ファイルでは 0 のみ有効）。
    pub fn from_bytes(data: Vec<u8>, index: u32) -> Result<Font> {
        let dir = sfnt::parse_table_directory(&data, index)?;

        let head_bytes = dir.require(&data, b"head")?;
        let head = tables::parse_head(head_bytes)?;

        let maxp_bytes = dir.require(&data, b"maxp")?;
        let num_glyphs = tables::parse_maxp_num_glyphs(maxp_bytes)?;

        let hhea_bytes = dir.require(&data, b"hhea")?;
        let hhea = tables::parse_hhea(hhea_bytes)?;

        let hmtx_bytes = dir.require(&data, b"hmtx")?;
        let hmtx_advances = tables::parse_hmtx_advances(hmtx_bytes, hhea.number_of_h_metrics)?;

        let os2 = dir.find_slice(&data, b"OS/2")?.and_then(tables::parse_os2);

        let family_name = dir
            .find_slice(&data, b"name")?
            .and_then(tables::parse_family_name)
            .unwrap_or_default();

        let cmap_bytes = dir.require(&data, b"cmap")?;
        let cmap = cmap::parse_cmap(cmap_bytes)?;

        let loca_bytes = dir.require(&data, b"loca")?;
        let loca = glyf::parse_loca(loca_bytes, num_glyphs, head.index_to_loc_format == 1)?;
        ensure!(
            loca.len() == num_glyphs as usize + 1,
            "loca テーブルの要素数が numGlyphs と一致しません"
        );

        let (glyf_offset, glyf_length) = dir
            .find(b"glyf")
            .ok_or_else(|| anyhow::anyhow!("必須テーブル 'glyf' がありません"))?;
        // 存在チェックだけ先にしておく（実際のスライスは outline() 呼び出し時に取る）
        sfnt::table_slice(&data, glyf_offset, glyf_length)?;

        let (ascent, descent, line_gap) = match &os2 {
            Some(o) if o.use_typo_metrics() => (o.typo_ascender, o.typo_descender, o.typo_line_gap),
            _ => (hhea.ascender, hhea.descender, hhea.line_gap),
        };

        Ok(Font {
            data,
            family_name,
            units_per_em: head.units_per_em,
            ascent,
            descent,
            line_gap,
            num_glyphs,
            loca,
            glyf_offset,
            glyf_length,
            hmtx_advances,
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
        let glyf_bytes = sfnt::table_slice(&self.data, self.glyf_offset, self.glyf_length)?;
        glyf::outline_for_glyph(glyf_bytes, &self.loca, glyph, self.num_glyphs)
    }
}

// テスト専用: `registry`/`text` の W1 テストが手組みフォントを再利用するため `pub(crate)`。
#[cfg(test)]
pub(crate) mod tests;
