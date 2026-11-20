//! gen-icon — アイコン埋め込み機能用の画像を手続き生成する。
//! `paint.rs` の SDF ベース図形と `noiria-core` の自作 PNG エンコーダのみで完結し、
//! 外部の画像アセットは一切使わない。
//!
//! 使い方:
//!   cargo run -p noiria-tools --bin gen-icon -- engine
//!     → crates/noiria-app/assets/icon.png (256x256, 実行時ウィンドウアイコンの既定値として埋め込む)
//!     → crates/noiria-app/assets/icon.ico (16/32/48 は BMP DIB, 256 は PNG 埋め込み。
//!       noiria-app/build.rs が .res へ変換して exe リソースにリンクする)
//!   cargo run -p noiria-tools --bin gen-icon -- sample
//!     → games/sample/icon.png (256x256, サンプルゲーム個別のウィンドウアイコン)

use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Result};
use image::RgbaImage;
use noiria_tools::paint::{self, Color};

/// 角丸バッジ（マージン 16px, 半径 48px）を上下グラデーションで塗る。
fn fill_badge(img: &mut RgbaImage, top: [u8; 3], bottom: [u8; 3]) {
    let margin = 16.0;
    let w = img.width() as f32;
    let h = img.height() as f32;
    let (x, y, bw, bh) = (margin, margin, w - margin * 2.0, h - margin * 2.0);
    let (cx, cy, half_w, half_h, radius) = (x + bw / 2.0, y + bh / 2.0, bw / 2.0, bh / 2.0, 48.0);
    for yy in 0..img.height() {
        let t = yy as f32 / (h - 1.0).max(1.0);
        let row_color: Color = [
            paint::lerp(top[0] as f32, bottom[0] as f32, t).round() as u8,
            paint::lerp(top[1] as f32, bottom[1] as f32, t).round() as u8,
            paint::lerp(top[2] as f32, bottom[2] as f32, t).round() as u8,
            255,
        ];
        for xx in 0..img.width() {
            let d = paint::sdf_round_rect(
                xx as f32 + 0.5,
                yy as f32 + 0.5,
                cx,
                cy,
                half_w,
                half_h,
                radius,
            );
            let coverage = (0.5 - d).clamp(0.0, 1.0);
            if coverage > 0.0 {
                paint::blend_over(img, xx as i32, yy as i32, row_color, coverage);
            }
        }
    }
}

/// エンジン既定アイコンの図案: 太字の "N" モノグラム（3 本の丸端線分）。
fn draw_n_glyph(img: &mut RgbaImage, color: Color) {
    let thickness = 34.0;
    paint::add_radial_glow(img, 128.0, 122.0, 108.0, [255, 255, 255], 0.10);
    paint::aa_line(img, 82.0, 72.0, 82.0, 184.0, thickness, color); // 左縦棒
    paint::aa_line(img, 174.0, 72.0, 174.0, 184.0, thickness, color); // 右縦棒
    paint::aa_line(img, 82.0, 72.0, 174.0, 184.0, thickness, color); // 斜め
}

/// サンプルゲーム用の図案: きらめきバッジ（gen_w3_icon.rs のスパーク意匠を拡大流用）。
fn draw_spark_glyph(img: &mut RgbaImage) {
    let (cx, cy) = (128.0, 128.0);
    paint::add_radial_glow(img, cx, cy, 120.0, [255, 214, 140], 0.35);
    paint::aa_circle(img, cx, cy, 66.0, [255, 226, 150, 255]);
    paint::aa_circle(img, cx, cy, 34.0, [255, 250, 235, 255]);
    paint::aa_rounded_rect_stroke(
        img,
        cx - 66.0,
        cy - 66.0,
        132.0,
        132.0,
        66.0,
        7.0,
        [140, 96, 30, 220],
    );
}

/// 最近傍リサイズ（straight RGBA8, `src_w*src_h*4` → `dst_w*dst_h*4`）。
fn nearest_resize(src_w: u32, src_h: u32, src: &[u8], dst_w: u32, dst_h: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dst_w * dst_h * 4) as usize];
    for y in 0..dst_h {
        let sy = (y * src_h) / dst_h.max(1);
        for x in 0..dst_w {
            let sx = (x * src_w) / dst_w.max(1);
            let si = ((sy * src_w + sx) * 4) as usize;
            let di = ((y * dst_w + x) * 4) as usize;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    out
}

/// Win32 の ICONIMAGE 形式（BITMAPINFOHEADER 40B + XOR 32bpp BGRA bottom-up +
/// AND マスク 1bpp bottom-up・4 バイト境界パディング）。`.ico`/`.res` の実体そのもの。
/// AND マスクは全 0 にし、透過はアルファチャンネルだけで表現する（Vista+ の標準的なやり方）。
/// `crates/noiria-app/src/icon.rs::build_dib_icon_image` に同一ロジックの複製がある
/// （クレート境界をまたぐため意図的に複製。詳細は implementation-notes.md 参照）。
fn build_dib_icon_image(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(40 + (w * h * 4) as usize);
    out.extend_from_slice(&40u32.to_le_bytes()); // biSize
    out.extend_from_slice(&(w as i32).to_le_bytes()); // biWidth
    out.extend_from_slice(&((h * 2) as i32).to_le_bytes()); // biHeight = XOR+AND合計
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    out.extend_from_slice(&0u32.to_le_bytes()); // biSizeImage
    out.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
    out.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant
    for y in (0..h).rev() {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let (r, g, b, a) = (rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]);
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    let row_bytes = (w as usize).div_ceil(32) * 4;
    let and_row = vec![0u8; row_bytes];
    for _ in 0..h {
        out.extend_from_slice(&and_row);
    }
    out
}

/// ICONDIR + ICONDIRENTRY[] + 画像実体、を組み立てて `.ico` バイト列にする。
/// `images`: (幅, 高さ, 実体バイト列)。256px はそのまま PNG バイト列を渡す
/// （幅/高さフィールドは ICONDIRENTRY の慣習どおり 256 は 0 として書く）。
fn build_ico(images: &[(u32, u32, Vec<u8>)]) -> Vec<u8> {
    let count = images.len() as u16;
    let mut header = Vec::new();
    header.extend_from_slice(&0u16.to_le_bytes()); // reserved
    header.extend_from_slice(&1u16.to_le_bytes()); // type = 1 (icon)
    header.extend_from_slice(&count.to_le_bytes());

    let mut entries = Vec::new();
    let mut data = Vec::new();
    let mut offset = 6u32 + 16u32 * images.len() as u32;
    for (w, h, bytes) in images {
        let bw = if *w >= 256 { 0u8 } else { *w as u8 };
        let bh = if *h >= 256 { 0u8 } else { *h as u8 };
        entries.push(bw);
        entries.push(bh);
        entries.push(0); // colorCount
        entries.push(0); // reserved
        entries.extend_from_slice(&1u16.to_le_bytes()); // planes
        entries.extend_from_slice(&32u16.to_le_bytes()); // bitcount
        entries.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        entries.extend_from_slice(&offset.to_le_bytes());
        offset += bytes.len() as u32;
        data.extend_from_slice(bytes);
    }

    let mut out = header;
    out.extend_from_slice(&entries);
    out.extend_from_slice(&data);
    out
}

fn gen_engine_icon() -> Result<()> {
    let mut img = RgbaImage::new(256, 256);
    fill_badge(&mut img, [0x2C, 0x25, 0x60], [0x6A, 0x4F, 0xC4]);
    draw_n_glyph(&mut img, [245, 240, 255, 255]);
    paint::vignette(&mut img, 0.18);

    let (w, h) = (img.width(), img.height());
    let raw = img.into_raw();
    let png_bytes = noiria_core::codec::png::encode(w, h, &raw);

    let app_dir = PathBuf::from("crates/noiria-app/assets");
    fs::create_dir_all(&app_dir)?;
    let png_path = app_dir.join("icon.png");
    fs::write(&png_path, &png_bytes)?;

    let mut images = Vec::new();
    for size in [16u32, 32, 48] {
        let resized = nearest_resize(w, h, &raw, size, size);
        let dib = build_dib_icon_image(size, size, &resized);
        images.push((size, size, dib));
    }
    images.push((256, 256, png_bytes));
    let ico = build_ico(&images);
    let ico_path = app_dir.join("icon.ico");
    fs::write(&ico_path, &ico)?;

    println!("生成しました: {}", png_path.display());
    println!("生成しました: {}", ico_path.display());
    Ok(())
}

fn gen_sample_icon() -> Result<()> {
    let mut img = RgbaImage::new(256, 256);
    fill_badge(&mut img, [0x7A, 0x3B, 0x12], [0xE0, 0x8A, 0x3C]);
    draw_spark_glyph(&mut img);
    paint::vignette(&mut img, 0.14);

    let (w, h) = (img.width(), img.height());
    let png_bytes = noiria_core::codec::png::encode(w, h, &img.into_raw());
    let path = PathBuf::from("games/sample/icon.png");
    fs::write(&path, &png_bytes)?;
    println!("生成しました: {}", path.display());
    Ok(())
}

fn main() -> Result<()> {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "engine".to_string());
    match mode.as_str() {
        "engine" => gen_engine_icon(),
        "sample" => gen_sample_icon(),
        other => bail!("不明なモードです（engine|sample のいずれかを指定してください）: {other}"),
    }
}
