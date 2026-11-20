//! F2 グリフラスタライザの目視確認用ダンプ。
//! システムフォント（msgothic.ttc 等、見つかった最初のもの）から「永あA漢」を
//! 48px でラスタライズし、横に並べた 1 枚の PNG を `target/dump/font_raster.png`
//! へ書き出す。フォントが1つも見つからない環境ではメッセージを出して終了する。
//!
//! 実行: cargo run -p noiria-core --example font_raster_dump
//! （ウィンドウ・音は一切出さない。PNG ファイルを書き出すだけ）

use std::path::Path;

use noiria_core::assets::save_png;
use noiria_core::font::{rasterize, Font, GlyphMask};
use noiria_core::pixmap::Pixmap;

const CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msgothic.ttc",
    r"C:\Windows\Fonts\YuGothM.ttc",
    r"C:\Windows\Fonts\meiryo.ttc",
    r"C:\Windows\Fonts\arial.ttf",
];

const TEXT: &str = "永あA漢";
const PX_PER_EM: f32 = 48.0;
const MARGIN: i32 = 16;
const GLYPH_GAP: i32 = 10;

fn load_first_available() -> Option<(String, Font)> {
    for path in CANDIDATES {
        let p = Path::new(path);
        if !p.exists() {
            continue;
        }
        match std::fs::read(p)
            .ok()
            .and_then(|bytes| Font::from_bytes(bytes, 0).ok())
        {
            Some(font) => return Some(((*path).to_string(), font)),
            None => continue,
        }
    }
    None
}

/// 8bit カバレッジマスクを白文字（straight RGBA）の Pixmap に変換する。
fn mask_to_white_pixmap(mask: &GlyphMask) -> Pixmap {
    let mut rgba = Vec::with_capacity(mask.data.len() * 4);
    for &a in &mask.data {
        rgba.extend_from_slice(&[255, 255, 255, a]);
    }
    Pixmap::from_rgba_straight(mask.width, mask.height, &rgba)
}

fn main() {
    let Some((path, font)) = load_first_available() else {
        println!(
            "システムフォントが見つからないため font_raster_dump をスキップします \
             (候補: {CANDIDATES:?})。Windows 環境で実行してください。"
        );
        return;
    };
    println!("フォント使用: {path} ({})", font.family_name());

    let scale = PX_PER_EM / font.units_per_em() as f32;

    // 1パス目: 全グリフをラスタライズし、pen_x（横位置）とベースラインからの
    // 上下はみ出し量を集計してキャンバスサイズを決める（アクセント等でフォントの
    // ascent/descent を超えるグリフがあっても切れないようにする）。
    struct Glyph {
        mask: GlyphMask,
        pen_x: i32,
    }
    let mut glyphs = Vec::new();
    let mut pen_x = 0i32;
    let mut top_extent = 0i32; // ベースラインから上方向の最大はみ出し
    let mut bottom_extent = 0i32; // ベースラインから下方向の最大はみ出し

    for ch in TEXT.chars() {
        let Some(gid) = font.glyph_id(ch).filter(|&g| g != 0) else {
            println!("'{ch}' の glyph_id が見つからないためスキップします");
            continue;
        };
        let outline = match font.outline(gid) {
            Ok(o) => o,
            Err(e) => {
                println!("'{ch}' の outline 抽出に失敗: {e:#}");
                continue;
            }
        };
        let mask = rasterize(&outline, scale);
        let advance_px = (font.advance(gid) as f32 * scale).round() as i32;
        if mask.width > 0 && mask.height > 0 {
            top_extent = top_extent.max(mask.top);
            bottom_extent = bottom_extent.max(mask.height as i32 - mask.top);
            glyphs.push(Glyph { mask, pen_x });
        }
        pen_x += advance_px.max(1) + GLYPH_GAP;
    }

    if glyphs.is_empty() {
        println!("ラスタライズできたグリフが1つもありませんでした");
        return;
    }

    let baseline_y = MARGIN + top_extent;
    let canvas_w = (pen_x + MARGIN).max(1) as u32;
    let canvas_h = (MARGIN + top_extent + bottom_extent + MARGIN).max(1) as u32;
    let mut canvas = Pixmap::new(canvas_w, canvas_h);
    canvas.fill([18, 20, 28, 255]); // 目視しやすいダークネイビー背景

    // 2パス目: 実際に描画する。
    for g in &glyphs {
        let pixmap = mask_to_white_pixmap(&g.mask);
        let draw_x = MARGIN + g.pen_x + g.mask.left;
        let draw_y = baseline_y - g.mask.top;
        canvas.blit_over(&pixmap, draw_x, draw_y, 255);
    }

    let out_path = Path::new("target/dump/font_raster.png");
    match save_png(&canvas, out_path) {
        Ok(()) => {
            let full = out_path
                .canonicalize()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| out_path.display().to_string());
            println!("書き出し完了: {full} ({canvas_w}x{canvas_h})");
        }
        Err(e) => {
            println!("PNG 書き出しに失敗しました: {e:#}");
        }
    }
}
