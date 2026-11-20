//! `gen-assets` 用の小さな描画基盤。
//!
//! `image::RgbaImage` に対して、距離場（SDF）ベースのアンチエイリアス付き図形、
//! グラデーション合成、ディザリング、ビネット、ソフトシャドウを提供する。
//! 外部クレート・外部画像は一切使わず、すべて手続き的に計算する。

use image::{Rgba, RgbaImage};

pub type Color = [u8; 4];

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn smoothstep01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    [
        lerp(a[0] as f32, b[0] as f32, t).round().clamp(0.0, 255.0) as u8,
        lerp(a[1] as f32, b[1] as f32, t).round().clamp(0.0, 255.0) as u8,
        lerp(a[2] as f32, b[2] as f32, t).round().clamp(0.0, 255.0) as u8,
        lerp(a[3] as f32, b[3] as f32, t).round().clamp(0.0, 255.0) as u8,
    ]
}

// ---- ノイズ / ディザ ----

/// 決定論的な整数ハッシュ（0..1 を返す）。
fn hash_i(ix: i32, iy: i32) -> f32 {
    let mut h = (ix.wrapping_mul(374_761_393) ^ iy.wrapping_mul(668_265_263)) as u32;
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^= h >> 16;
    (h & 0xFFFF) as f32 / 65535.0
}

/// Interleaved Gradient Noise。白色雑音より低周波成分が少なく、
/// グラデーションのバンディング抑制用ディザとして目立ちにくい。
/// 戻り値は概ね [-1.0, 1.0]。
pub fn dither_noise(x: u32, y: u32) -> f32 {
    let xf = x as f32;
    let yf = y as f32;
    let v = (52.982_918 * (0.067_110_56 * xf + 0.005_837_15 * yf).fract()).fract();
    (v - 0.5) * 2.0
}

/// 格子補間によるスムースなバリューノイズ（0..1）。壁のムラ・雲の輪郭揺らぎ等に使う。
pub fn value_noise(x: f32, y: f32) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let (ix0, iy0) = (x0 as i32, y0 as i32);
    let v00 = hash_i(ix0, iy0);
    let v10 = hash_i(ix0 + 1, iy0);
    let v01 = hash_i(ix0, iy0 + 1);
    let v11 = hash_i(ix0 + 1, iy0 + 1);
    let sx = smoothstep01(fx);
    let sy = smoothstep01(fy);
    let a = lerp(v00, v10, sx);
    let b = lerp(v01, v11, sx);
    lerp(a, b, sy)
}

/// 2〜3 オクターブ重ねた fBm ノイズ（0..1 付近）。
pub fn fbm_noise(x: f32, y: f32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut norm = 0.0;
    for _ in 0..3 {
        sum += value_noise(x * freq, y * freq) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.1;
    }
    sum / norm
}

// ---- 基本合成 ----

/// src-over。`coverage` (0..1) は形状 AA カバレッジ、`color[3]` は素材自体の不透明度。
/// 背景が不透明でも透過でも正しく合成される。
pub fn blend_over(img: &mut RgbaImage, x: i32, y: i32, color: Color, coverage: f32) {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return;
    }
    let coverage = coverage.clamp(0.0, 1.0);
    if coverage <= 0.0 {
        return;
    }
    let src_a = (color[3] as f32 / 255.0) * coverage;
    if src_a <= 0.0 {
        return;
    }
    let px = img.get_pixel_mut(x as u32, y as u32);
    let dst_a = px[3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0001 {
        *px = Rgba([0, 0, 0, 0]);
        return;
    }
    let mut out = [0u8; 4];
    for i in 0..3 {
        let src_c = color[i] as f32;
        let dst_c = px[i] as f32;
        let out_c = (src_c * src_a + dst_c * dst_a * (1.0 - src_a)) / out_a;
        out[i] = out_c.round().clamp(0.0, 255.0) as u8;
    }
    out[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
    *px = Rgba(out);
}

/// 加算合成（グロー用）。アルファは変更しない（不透明キャンバス前提）。
pub fn blend_add(img: &mut RgbaImage, x: i32, y: i32, color: [u8; 3], amount: f32) {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return;
    }
    let amount = amount.max(0.0);
    if amount <= 0.0 {
        return;
    }
    let px = img.get_pixel_mut(x as u32, y as u32);
    for i in 0..3 {
        px[i] = (px[i] as f32 + color[i] as f32 * amount).clamp(0.0, 255.0) as u8;
    }
}

/// 単純な行方向 AA 水平スパン（x_left..x_right, 端の 1px だけカバレッジで滑らかにする）。
pub fn h_span_aa(img: &mut RgbaImage, y: i32, x_left: f32, x_right: f32, color: Color) {
    if x_right <= x_left {
        return;
    }
    let xl = x_left.floor() as i32 - 1;
    let xr = x_right.ceil() as i32 + 1;
    for x in xl..xr {
        let px_l = x as f32;
        let px_r = x as f32 + 1.0;
        let overlap = (px_r.min(x_right) - px_l.max(x_left)).clamp(0.0, 1.0);
        if overlap > 0.0 {
            blend_over(img, x, y, color, overlap);
        }
    }
}

// ---- グラデーション ----

/// 画像全面への垂直グラデーション + ディザ（バンディング防止）。
pub fn fill_vertical_gradient(img: &mut RgbaImage, top: [u8; 3], bottom: [u8; 3]) {
    let h = img.height();
    for y in 0..img.height() {
        let t = y as f32 / (h - 1).max(1) as f32;
        for x in 0..img.width() {
            let n = dither_noise(x, y);
            let r = (lerp(top[0] as f32, bottom[0] as f32, t) + n)
                .round()
                .clamp(0.0, 255.0) as u8;
            let g = (lerp(top[1] as f32, bottom[1] as f32, t) + n)
                .round()
                .clamp(0.0, 255.0) as u8;
            let b = (lerp(top[2] as f32, bottom[2] as f32, t) + n)
                .round()
                .clamp(0.0, 255.0) as u8;
            img.put_pixel(x, y, Rgba([r, g, b, 255]));
        }
    }
}

/// 放射状グロー（加算合成）。太陽・窓明かりの光暈に使う。
pub fn add_radial_glow(
    img: &mut RgbaImage,
    cx: f32,
    cy: f32,
    radius: f32,
    color: [u8; 3],
    max_amount: f32,
) {
    let x0 = (cx - radius).floor().max(0.0) as i32;
    let x1 = (cx + radius).ceil().min(img.width() as f32) as i32;
    let y0 = (cy - radius).floor().max(0.0) as i32;
    let y1 = (cy + radius).ceil().min(img.height() as f32) as i32;
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist >= radius {
                continue;
            }
            let t = 1.0 - dist / radius;
            let amount = t * t * max_amount;
            blend_add(img, x, y, color, amount);
        }
    }
}

// ---- 図形 ----

/// アンチエイリアス付き塗りつぶし円（距離場ベース）。
pub fn aa_circle(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, color: Color) {
    let x0 = (cx - r - 1.0).floor().max(0.0) as i32;
    let x1 = (cx + r + 1.0).ceil().min(img.width() as f32) as i32;
    let y0 = (cy - r - 1.0).floor().max(0.0) as i32;
    let y1 = (cy + r + 1.0).ceil().min(img.height() as f32) as i32;
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let coverage = (r + 0.5 - dist).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend_over(img, x, y, color, coverage);
            }
        }
    }
}

/// 柔らかい縁を持つ楕円（雲・チーク等）。`feather` は 0..1 で縁のぼかし幅（半径比）。
pub fn aa_ellipse_soft(
    img: &mut RgbaImage,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    color: Color,
    feather: f32,
) {
    if rx <= 0.0 || ry <= 0.0 {
        return;
    }
    let pad = 1.0;
    let x0 = (cx - rx - pad).floor().max(0.0) as i32;
    let x1 = (cx + rx + pad).ceil().min(img.width() as f32) as i32;
    let y0 = (cy - ry - pad).floor().max(0.0) as i32;
    let y1 = (cy + ry + pad).ceil().min(img.height() as f32) as i32;
    let feather = feather.clamp(0.001, 1.0);
    let inner = 1.0 - feather;
    for y in y0..y1 {
        for x in x0..x1 {
            let nx = (x as f32 + 0.5 - cx) / rx;
            let ny = (y as f32 + 0.5 - cy) / ry;
            let d = (nx * nx + ny * ny).sqrt();
            let coverage = if d <= inner {
                1.0
            } else {
                (1.0 - (d - inner) / feather).clamp(0.0, 1.0)
            };
            if coverage > 0.0 {
                blend_over(img, x, y, color, coverage);
            }
        }
    }
}

/// 角丸矩形の符号付き距離（外側が正）。
pub fn sdf_round_rect(
    px: f32,
    py: f32,
    cx: f32,
    cy: f32,
    half_w: f32,
    half_h: f32,
    radius: f32,
) -> f32 {
    let radius = radius.min(half_w).min(half_h).max(0.0);
    let qx = (px - cx).abs() - (half_w - radius);
    let qy = (py - cy).abs() - (half_h - radius);
    let ax = qx.max(0.0);
    let ay = qy.max(0.0);
    (ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - radius
}

/// アンチエイリアス付き塗りつぶし角丸矩形。`radius=0` で普通の矩形。
pub fn aa_rounded_rect(
    img: &mut RgbaImage,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    color: Color,
) {
    let cx = x + w / 2.0;
    let cy = y + h / 2.0;
    let half_w = w / 2.0;
    let half_h = h / 2.0;
    let x0 = (x - 1.0).floor().max(0.0) as i32;
    let x1 = (x + w + 1.0).ceil().min(img.width() as f32) as i32;
    let y0 = (y - 1.0).floor().max(0.0) as i32;
    let y1 = (y + h + 1.0).ceil().min(img.height() as f32) as i32;
    for yy in y0..y1 {
        for xx in x0..x1 {
            let d = sdf_round_rect(
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
                blend_over(img, xx, yy, color, coverage);
            }
        }
    }
}

/// アンチエイリアス付き角丸矩形の輪郭線（枠）。
#[allow(clippy::too_many_arguments)]
pub fn aa_rounded_rect_stroke(
    img: &mut RgbaImage,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    thickness: f32,
    color: Color,
) {
    let cx = x + w / 2.0;
    let cy = y + h / 2.0;
    let half_w = w / 2.0;
    let half_h = h / 2.0;
    let pad = thickness + 1.0;
    let x0 = (x - pad).floor().max(0.0) as i32;
    let x1 = (x + w + pad).ceil().min(img.width() as f32) as i32;
    let y0 = (y - pad).floor().max(0.0) as i32;
    let y1 = (y + h + pad).ceil().min(img.height() as f32) as i32;
    for yy in y0..y1 {
        for xx in x0..x1 {
            let d = sdf_round_rect(
                xx as f32 + 0.5,
                yy as f32 + 0.5,
                cx,
                cy,
                half_w,
                half_h,
                radius,
            )
            .abs()
                - thickness / 2.0;
            let coverage = (0.5 - d).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend_over(img, xx, yy, color, coverage);
            }
        }
    }
}

/// アンチエイリアス付き線分（カプセル形状）。
pub fn aa_line(
    img: &mut RgbaImage,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    thickness: f32,
    color: Color,
) {
    let pad = thickness + 1.0;
    let bx0 = (x0.min(x1) - pad).floor().max(0.0) as i32;
    let bx1 = (x0.max(x1) + pad).ceil().min(img.width() as f32) as i32;
    let by0 = (y0.min(y1) - pad).floor().max(0.0) as i32;
    let by1 = (y0.max(y1) + pad).ceil().min(img.height() as f32) as i32;
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len2 = (dx * dx + dy * dy).max(1e-6);
    for y in by0..by1 {
        for x in bx0..bx1 {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let t = (((px - x0) * dx + (py - y0) * dy) / len2).clamp(0.0, 1.0);
            let cx = x0 + dx * t;
            let cy = y0 + dy * t;
            let ddx = px - cx;
            let ddy = py - cy;
            let dist = (ddx * ddx + ddy * ddy).sqrt();
            let coverage = (thickness / 2.0 + 0.5 - dist).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend_over(img, x, y, color, coverage);
            }
        }
    }
}

/// 二次ベジエ曲線をカプセル線分の連結で近似描画する（前髪の房、まつ毛、電線のたわみ等）。
pub fn aa_bezier(
    img: &mut RgbaImage,
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    thickness: f32,
    color: Color,
    segments: u32,
) {
    let mut prev = p0;
    for i in 1..=segments {
        let t = i as f32 / segments as f32;
        let mt = 1.0 - t;
        let x = mt * mt * p0.0 + 2.0 * mt * t * p1.0 + t * t * p2.0;
        let y = mt * mt * p0.1 + 2.0 * mt * t * p1.1 + t * t * p2.1;
        aa_line(img, prev.0, prev.1, x, y, thickness, color);
        prev = (x, y);
    }
}

// ---- 仕上げ効果 ----

/// 四隅を僅かに暗くするビネット。
pub fn vignette(img: &mut RgbaImage, strength: f32) {
    let w = img.width() as f32;
    let h = img.height() as f32;
    let cx = w / 2.0;
    let cy = h / 2.0;
    let max_d = (cx * cx + cy * cy).sqrt();
    for y in 0..img.height() {
        for x in 0..img.width() {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let d = ((dx * dx + dy * dy).sqrt() / max_d).clamp(0.0, 1.0);
            let factor = 1.0 - strength * d.powf(2.2);
            let px = img.get_pixel_mut(x, y);
            for i in 0..3 {
                px[i] = (px[i] as f32 * factor.max(0.0)).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// 矩形の柔らかい落ち影（ぼかし半径 = blur）。矩形本体より先に描画すること。
pub fn soft_shadow_rect(
    img: &mut RgbaImage,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    blur: f32,
    color: Color,
) {
    let half_w = w / 2.0;
    let half_h = h / 2.0;
    let cx = x + half_w;
    let cy = y + half_h;
    let pad = blur + 1.0;
    let x0 = (x - pad).floor().max(0.0) as i32;
    let x1 = (x + w + pad).ceil().min(img.width() as f32) as i32;
    let y0 = (y - pad).floor().max(0.0) as i32;
    let y1 = (y + h + pad).ceil().min(img.height() as f32) as i32;
    let src_a = color[3] as f32 / 255.0;
    for yy in y0..y1 {
        for xx in x0..x1 {
            let px = xx as f32 + 0.5;
            let py = yy as f32 + 0.5;
            let dx = ((px - cx).abs() - half_w).max(0.0);
            let dy = ((py - cy).abs() - half_h).max(0.0);
            let dist = (dx * dx + dy * dy).sqrt();
            let coverage = if blur <= 0.0 {
                if dist <= 0.0 {
                    1.0
                } else {
                    0.0
                }
            } else {
                (1.0 - dist / blur).clamp(0.0, 1.0)
            };
            if coverage > 0.0 {
                blend_over(img, xx, yy, color, coverage * src_a);
            }
        }
    }
}
