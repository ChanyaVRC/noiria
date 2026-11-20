//! gen-w4-button — W4 (UI・ボタン・クリック制御) の [button] デモ用に、
//! 正常/ホバー/押下の 3 状態が横に並んだボタン画像を 1 枚だけ手続き生成する。
//! gen-w3-icon と同じ理由（既存アセット群の再生成に伴う意図しない差分を避ける）で
//! 独立したバイナリにする。
//! 使い方: cargo run -p noiria-tools --bin gen-w4-button -- games/sample/assets/fg/btn_menu.png

use std::path::PathBuf;

use anyhow::Result;
use image::RgbaImage;
use noiria_tools::paint::{aa_circle, aa_rounded_rect, aa_rounded_rect_stroke};

/// 1 状態ぶんの矩形 (幅 120x44) を描く。normal/over/pressed で塗り色と
/// 内側の小さな丸(状態インジケータ)の色を変えるだけの簡素な意匠。
fn draw_state(img: &mut RgbaImage, ox: f32, fill: [u8; 4], dot: [u8; 4]) {
    let (w, h) = (120.0f32, 44.0);
    aa_rounded_rect(img, ox + 2.0, 2.0, w - 4.0, h - 4.0, 10.0, fill);
    aa_rounded_rect_stroke(
        img,
        ox + 2.0,
        2.0,
        w - 4.0,
        h - 4.0,
        10.0,
        1.6,
        [20, 20, 30, 200],
    );
    aa_circle(img, ox + 20.0, h / 2.0, 6.0, dot);
}

fn main() -> Result<()> {
    let path: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "games/sample/assets/fg/btn_menu.png".to_string())
        .into();

    let (state_w, h) = (120u32, 44u32);
    let mut img = RgbaImage::new(state_w * 3, h);
    draw_state(&mut img, 0.0, [70, 74, 100, 235], [150, 155, 190, 255]); // normal
    draw_state(
        &mut img,
        state_w as f32,
        [90, 120, 190, 235],
        [220, 235, 255, 255],
    ); // over
    draw_state(
        &mut img,
        (state_w * 2) as f32,
        [255, 196, 90, 235],
        [255, 250, 230, 255],
    ); // pressed

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    img.save(&path)?;
    println!("生成しました: {}", path.display());
    Ok(())
}
