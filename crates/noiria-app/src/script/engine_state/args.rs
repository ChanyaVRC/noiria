//! `EngineState` と V8/tsint ops の間で受け渡す引数・結果・テキスト蓄積コンテナ。

use std::collections::HashMap;

use noiria_core::text::{LocateArgs, TextAlign, TextEngine, TextSegment, TextStyle};
use serde::{Deserialize, Serialize};

use crate::script::numconv::{
    de_opt_coord, de_opt_i32, de_opt_u32, de_opt_u8, de_time_ms, de_u32, to_u8,
};

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub(crate) struct LayerProps {
    #[serde(deserialize_with = "de_opt_coord")]
    pub(crate) x: Option<i32>,
    #[serde(deserialize_with = "de_opt_coord")]
    pub(crate) y: Option<i32>,
    #[serde(deserialize_with = "de_opt_i32")]
    pub(crate) z: Option<i32>,
    #[serde(deserialize_with = "de_opt_u8")]
    pub(crate) opacity: Option<u8>,
    pub(crate) visible: Option<bool>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct MoveArgs {
    #[serde(deserialize_with = "de_opt_coord")]
    pub(crate) to_x: Option<i32>,
    #[serde(deserialize_with = "de_opt_coord")]
    pub(crate) to_y: Option<i32>,
    #[serde(deserialize_with = "de_opt_u8")]
    pub(crate) to_opacity: Option<u8>,
    #[serde(deserialize_with = "de_time_ms")]
    pub(crate) time_ms: u64,
    pub(crate) accel: f32,
}

/// `layer_blit_image` の clip= 矩形（画像ファイル内の切り出し範囲）。省略時は画像全体。
#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) struct BlitClip {
    #[serde(deserialize_with = "de_u32")]
    pub(crate) x: u32,
    #[serde(deserialize_with = "de_u32")]
    pub(crate) y: u32,
    #[serde(deserialize_with = "de_u32")]
    pub(crate) w: u32,
    #[serde(deserialize_with = "de_u32")]
    pub(crate) h: u32,
}

/// [image]（W5 拡張）の clip=/flipud=/fliplr=/grayscale= 属性。`layer_load_image` の拡張版
/// `layer_load_image_ex` が使う。適用順は clip 抽出 → 左右反転 → 上下反転 → グレースケール化。
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct ImageLoadOpts {
    pub(crate) clip: Option<BlitClip>,
    pub(crate) flip_h: bool,
    pub(crate) flip_v: bool,
    pub(crate) grayscale: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TransArgs {
    pub(crate) method: String,
    #[serde(default, deserialize_with = "de_time_ms")]
    pub(crate) time_ms: u64,
    #[serde(default)]
    pub(crate) rule: Option<String>,
    #[serde(default)]
    pub(crate) vague: Option<f32>,
    /// [trans method=scroll]（W5）: 新ページが入ってくる辺（既定 left）。
    #[serde(default)]
    pub(crate) from: Option<String>,
    /// [trans method=scroll]（W5）: stayfore|staynone|staybase を受理するが、描画上は
    /// 区別しない（`Pixmap::scroll_from` のコメント参照）。値自体は保持だけして未使用のため
    /// dead_code を明示的に許可する。
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) stay: Option<String>,
}

#[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
fn rgba_u8(c: [f64; 4]) -> [u8; 4] {
    c.map(to_u8)
}

/// V8 の `op_text_style` が受ける `TextStyle`。`noiria_core::text::TextStyle` を直接 serde で受けると
/// 整数フィールドが `f64 as T`（切り捨て）になり tsint（四捨五入）と食い違うので、数値は f64 で受けて
/// `numconv` の規則で落とす。省略したフィールドは既定値のまま。
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
#[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
pub(crate) struct TextStyleArgs {
    pub(crate) font_family: Option<String>,
    pub(crate) font_size: Option<f32>,
    pub(crate) line_height: Option<f32>,
    pub(crate) color: Option<[f64; 4]>,
    pub(crate) outline_color: Option<[f64; 4]>,
    #[serde(deserialize_with = "de_opt_u32")]
    pub(crate) outline_width: Option<u32>,
    #[serde(deserialize_with = "de_opt_u32")]
    pub(crate) box_w: Option<u32>,
    #[serde(deserialize_with = "de_opt_u32")]
    pub(crate) box_h: Option<u32>,
    #[serde(deserialize_with = "de_opt_i32")]
    pub(crate) margin_x: Option<i32>,
    #[serde(deserialize_with = "de_opt_i32")]
    pub(crate) margin_y: Option<i32>,
    pub(crate) align: Option<TextAlign>,
}

#[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
impl TextStyleArgs {
    pub(crate) fn into_style(self) -> TextStyle {
        let mut s = TextStyle::default();
        if let Some(v) = self.font_family {
            s.font_family = v;
        }
        if let Some(v) = self.font_size {
            s.font_size = v;
        }
        if let Some(v) = self.line_height {
            s.line_height = v;
        }
        if let Some(v) = self.color {
            s.color = rgba_u8(v);
        }
        if let Some(v) = self.outline_color {
            s.outline_color = rgba_u8(v);
        }
        if let Some(v) = self.outline_width {
            s.outline_width = v;
        }
        if let Some(v) = self.box_w {
            s.box_w = v;
        }
        if let Some(v) = self.box_h {
            s.box_h = v;
        }
        if let Some(v) = self.margin_x {
            s.margin_x = v;
        }
        if let Some(v) = self.margin_y {
            s.margin_y = v;
        }
        if let Some(v) = self.align {
            s.align = v;
        }
        s
    }
}

/// V8 の `op_text_print` が受ける 1 セグメント（`TextStyleArgs` と同じ理由で色だけ f64 で受ける）。
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
#[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
pub(crate) struct TextSegmentArgs {
    pub(crate) text: String,
    pub(crate) color: Option<[f64; 4]>,
    pub(crate) size: Option<f32>,
    pub(crate) bold: Option<bool>,
    pub(crate) edge_color: Option<[f64; 4]>,
    pub(crate) ruby: Option<String>,
    pub(crate) locate: Option<LocateArgs>,
    pub(crate) indent: Option<bool>,
    pub(crate) inline_image: Option<String>,
    pub(crate) font_family: Option<String>,
}

#[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
impl TextSegmentArgs {
    pub(crate) fn into_segment(self) -> TextSegment {
        TextSegment {
            text: self.text,
            color: self.color.map(rgba_u8),
            size: self.size,
            bold: self.bold,
            edge_color: self.edge_color.map(rgba_u8),
            ruby: self.ruby,
            locate: self.locate,
            indent: self.indent,
            inline_image: self.inline_image,
            font_family: self.font_family,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PrintResult {
    pub(crate) total: usize,
    pub(crate) added: usize,
}

/// テキストレイアウトエンジンとレイヤごとの蓄積テキスト（旧 `state::Texts`）。
pub(crate) struct Texts {
    pub(crate) engine: TextEngine,
    pub(crate) styles: HashMap<u32, TextStyle>,
    pub(crate) segments: HashMap<u32, Vec<TextSegment>>,
    /// 直近レイアウトの総グリフ数（追記時の再レイアウトを避けるキャッシュ）
    pub(crate) glyph_counts: HashMap<u32, usize>,
}

impl Texts {
    pub(crate) fn new() -> Self {
        Self {
            engine: TextEngine::new(),
            styles: HashMap::new(),
            segments: HashMap::new(),
            glyph_counts: HashMap::new(),
        }
    }
}

impl Default for Texts {
    fn default() -> Self {
        Self::new()
    }
}
