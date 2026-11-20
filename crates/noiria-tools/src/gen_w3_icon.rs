//! gen-w3-icon — W3 (複数メッセージレイヤ) の [graph] インライン画像デモ用に、
//! 小さな装飾アイコンを 1 枚だけ手続き生成する。既存の `gen-assets` (main.rs) とは
//! 独立したバイナリにして、既存アセット群の再生成に伴う意図しない差分を避ける。
//! 使い方: cargo run -p noiria-tools --bin gen-w3-icon -- games/sample/assets/fg/icon_spark.png

use std::path::PathBuf;

use anyhow::Result;
use image::RgbaImage;
use noiria_tools::paint::{aa_circle, aa_rounded_rect_stroke};

fn main() -> Result<()> {
    let path: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "games/sample/assets/fg/icon_spark.png".to_string())
        .into();

    // [graph] のデモに使う、メッセージ本文の行になじむ小さな丸バッジ (28x28, 透過)。
    let (w, h) = (28u32, 28u32);
    let mut img = RgbaImage::new(w, h);
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    aa_circle(&mut img, cx, cy, 13.0, [255, 216, 120, 255]);
    aa_circle(&mut img, cx, cy, 7.0, [255, 250, 230, 255]);
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

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    img.save(&path)?;
    println!("生成しました: {}", path.display());
    Ok(())
}
