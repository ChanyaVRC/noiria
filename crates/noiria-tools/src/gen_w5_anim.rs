//! gen-w5-anim — W5 (トランジション/アニメ/画像操作) の [animstart]/[wa]/[animstop] デモ用に、
//! アイコンの点滅アニメーション 2 フレーム（開/閉）を手続き生成する。gen-w3-icon/gen-w4-button
//! と同じ理由（既存アセット群の再生成に伴う意図しない差分を避ける）で独立したバイナリにする。
//! 使い方: cargo run -p noiria-tools --bin gen-w5-anim -- games/sample/assets/fg

use std::path::PathBuf;

use anyhow::Result;
use image::RgbaImage;
use noiria_tools::paint::{aa_circle, aa_rounded_rect_stroke};

/// icon_spark.png (W3) と同じ 28x28 の丸バッジ意匠だが、`bright` で内側の明るさを
/// 変える（true=開いた目=明るい黄、false=閉じた目=暗いオレンジ）ことで、[animstart] の
/// フレーム差し替えが目視で分かるようにする。
fn draw_frame(bright: bool) -> RgbaImage {
    let (w, h) = (28u32, 28u32);
    let mut img = RgbaImage::new(w, h);
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let outer = if bright {
        [255, 216, 120, 255]
    } else {
        [140, 90, 40, 255]
    };
    let inner = if bright {
        [255, 250, 230, 255]
    } else {
        [200, 150, 90, 255]
    };
    aa_circle(&mut img, cx, cy, 13.0, outer);
    aa_circle(&mut img, cx, cy, if bright { 7.0 } else { 3.0 }, inner);
    aa_rounded_rect_stroke(
        &mut img,
        cx - 13.0,
        cy - 13.0,
        26.0,
        26.0,
        13.0,
        1.4,
        [140, 96, 30, 200],
    );
    img
}

fn main() -> Result<()> {
    let dir: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "games/sample/assets/fg".to_string())
        .into();
    std::fs::create_dir_all(&dir)?;

    let a = draw_frame(true);
    let b = draw_frame(false);
    let path_a = dir.join("icon_blink_a.png");
    let path_b = dir.join("icon_blink_b.png");
    a.save(&path_a)?;
    b.save(&path_b)?;
    println!("生成しました: {} / {}", path_a.display(), path_b.display());
    Ok(())
}
