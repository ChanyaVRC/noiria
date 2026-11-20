//! 目視確認用のプレビュー合成（`gen-demo-assets --preview`）。製品アセットには含まれない。

use image::imageops::{self, FilterType};
use image::{Rgba, RgbaImage};

use super::chara::{render_sprites_styled, Expr, Style, Who};

const GLYPHS: &[(char, [u8; 7])] = &[
    ('A', [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    ('B', [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E]),
    ('C', [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E]),
    ('E', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F]),
    ('F', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10]),
    ('I', [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ('L', [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F]),
    ('M', [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11]),
    ('N', [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11]),
    ('P', [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10]),
    ('R', [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11]),
    ('T', [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
    ('U', [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    ('-', [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00]),
];

/// 5x7 ビットマップ文字（大文字のみ・必要な字だけ）で `s` を描く。
pub fn text(img: &mut RgbaImage, x: i32, y: i32, scale: i32, s: &str, color: Rgba<u8>) {
    let mut cx = x;
    for ch in s.chars() {
        if let Some((_, rows)) = GLYPHS.iter().find(|(c, _)| *c == ch) {
            for (ry, bits) in rows.iter().enumerate() {
                for rx in 0..5 {
                    if bits & (0x10 >> rx) != 0 {
                        for dy in 0..scale {
                            for dx in 0..scale {
                                let (px, py) = (cx + rx * scale + dx, y + ry as i32 * scale + dy);
                                if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                                    img.put_pixel(px as u32, py as u32, color);
                                }
                            }
                        }
                    }
                }
            }
        }
        cx += 6 * scale;
    }
}

fn sprite_of(who: Who, style: Style, e: Expr) -> RgbaImage {
    render_sprites_styled(who, style)
        .into_iter()
        .find(|(x, _)| *x == e)
        .map(|(_, i)| i)
        .unwrap()
}

/// 立ち絵を背景へ合成（ゲーム画面と同じ 1:1 配置なら `scale = 1.0`）。
pub fn place(bg: &mut RgbaImage, sprite: &RgbaImage, left: i32, top: i32, scale: f32) {
    if (scale - 1.0).abs() < 1e-3 {
        imageops::overlay(bg, sprite, left as i64, top as i64);
    } else {
        let w = (sprite.width() as f32 * scale) as u32;
        let h = (sprite.height() as f32 * scale) as u32;
        let s = imageops::resize(sprite, w, h, FilterType::Lanczos3);
        imageops::overlay(bg, &s, left as i64, top as i64);
    }
}

/// 3 画風を教室背景に並べた比較シート（各行: ひなた normal/smile、しおり normal/blush）。
pub fn style_candidates(bg: &RgbaImage) -> RgbaImage {
    let row_h = 470u32;
    let mut out = RgbaImage::new(1280, row_h * 3);
    for (i, (style, label)) in [(Style::A, "A  FLAT MINIMAL"), (Style::B, "B  PAPER CUT"), (Style::C, "C  TALL FLAT")]
        .into_iter()
        .enumerate()
    {
        let crop = imageops::crop_imm(bg, 0, 120, 1280, row_h).to_image();
        imageops::overlay(&mut out, &crop, 0, (i as u32 * row_h) as i64);
        let y0 = (i as u32 * row_h) as i32;
        let scale = 0.5;
        for (k, (who, e)) in [(Who::Hinata, Expr::Normal), (Who::Hinata, Expr::Smile), (Who::Shiori, Expr::Normal), (Who::Shiori, Expr::Blush)]
            .into_iter()
            .enumerate()
        {
            place(&mut out, &sprite_of(who, style, e), 190 + k as i32 * 270, y0 + 14, scale);
        }
        let mut words = label.split_whitespace();
        text(&mut out, 14, y0 + 14, 6, words.next().unwrap_or(""), Rgba([58, 34, 58, 255]));
        for (n, w) in words.enumerate() {
            text(&mut out, 14, y0 + 72 + n as i32 * 30, 3, w, Rgba([58, 34, 58, 255]));
        }
    }
    out
}

/// 4 表情を単色背景に並べたシート。
pub fn sheet(who: Who, style: Style) -> RgbaImage {
    let mut out = RgbaImage::from_pixel(2000, 900, Rgba([150, 168, 200, 255]));
    for (k, e) in [Expr::Normal, Expr::Smile, Expr::Blush, Expr::Sad].into_iter().enumerate() {
        place(&mut out, &sprite_of(who, style, e), k as i32 * 500, 0, 1.0);
    }
    out
}

/// ゲーム画面（1280x720）相当の合成。`slots` = (who, expr, left)。top は 60。
pub fn scene(bg: &RgbaImage, style: Style, slots: &[(Who, Expr, i32)]) -> RgbaImage {
    let mut out = bg.clone();
    for &(who, e, left) in slots {
        place(&mut out, &sprite_of(who, style, e), left, 60, 1.0);
    }
    out
}
