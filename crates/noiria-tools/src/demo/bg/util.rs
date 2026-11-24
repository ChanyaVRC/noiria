//! 背景描画の共通ヘルパ。ルール画像・アイコン用の乱数と星の光、フラット背景用のベタ塗りプリミティブ。

use super::*;

pub const W: usize = 1280;
pub const H: usize = 720;
pub type P = (f32, f32);

pub fn canvas() -> Canvas {
    Canvas::new(W, H)
}

// ------------------------------------------------------------
// 乱数
// ------------------------------------------------------------
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03 | 1)
    }
    pub fn u(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn f(&mut self) -> f32 {
        (self.u() >> 40) as f32 / (1u32 << 24) as f32
    }
    pub fn r(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
}

// ------------------------------------------------------------
fn bounds(cv: &Canvas, x0: f32, y0: f32, x1: f32, y1: f32) -> (usize, usize, usize, usize) {
    (
        x0.floor().max(0.0) as usize,
        (x1.ceil().min(cv.w as f32).max(0.0)) as usize,
        y0.floor().max(0.0) as usize,
        (y1.ceil().min(cv.h as f32).max(0.0)) as usize,
    )
}
// ------------------------------------------------------------
pub fn star_px(cv: &mut Canvas, x: f32, y: f32, sigma: f32, c: [f32; 3], amt: f32) {
    let ext = (sigma * 3.0).ceil() + 1.0;
    let k = 1.0 / (2.0 * sigma * sigma);
    let (xa, xb, ya, yb) = bounds(cv, x - ext, y - ext, x + ext + 1.0, y + ext + 1.0);
    for py in ya..yb {
        for px in xa..xb {
            let dx = px as f32 + 0.5 - x;
            let dy = py as f32 + 0.5 - y;
            let v = (-(dx * dx + dy * dy) * k).exp();
            if v > 0.003 {
                cv.add(px, py, c, v * amt);
            }
        }
    }
}

pub fn flare(cv: &mut Canvas, x: f32, y: f32, len: f32, c: [f32; 3], amt: f32, diag: bool) {
    let n = if diag { 4 } else { 2 };
    for k in 0..n {
        let ang = if k < 2 {
            k as f32 * std::f32::consts::FRAC_PI_2
        } else {
            std::f32::consts::FRAC_PI_4 + (k - 2) as f32 * std::f32::consts::FRAC_PI_2
        };
        let (s, co) = ang.sin_cos();
        let l = if k < 2 { len } else { len * 0.55 };
        let a = if k < 2 { amt } else { amt * 0.5 };
        let (xa, xb, ya, yb) = bounds(cv, x - l, y - l, x + l + 1.0, y + l + 1.0);
        for py in ya..yb {
            for px in xa..xb {
                let dx = px as f32 + 0.5 - x;
                let dy = py as f32 + 0.5 - y;
                let along = (dx * co + dy * s).abs() / l;
                let perp = (-dx * s + dy * co).abs();
                if along < 1.0 {
                    let v = (1.0 - along).powi(3) * (-perp * perp / 0.9).exp();
                    if v > 0.003 {
                        cv.add(px, py, c, v * a);
                    }
                }
            }
        }
    }
}
/// n 角星の頂点列（外半径 r_out、内半径 r_in、rot=0 で真上が頂点）。
pub fn star_pts(cx: f32, cy: f32, r_out: f32, r_in: f32, rot: f32, n: usize) -> Vec<P> {
    (0..n * 2)
        .map(|i| {
            let a = rot - std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / n as f32;
            let r = if i % 2 == 0 { r_out } else { r_in };
            (cx + a.cos() * r, cy + a.sin() * r)
        })
        .collect()
}

// ------------------------------------------------------------
// フラット背景用のベタ塗り
// ------------------------------------------------------------
pub fn pmask(cv: &Canvas, pts: &[P]) -> Mask {
    Mask::from_path(&Path::polygon(pts), (cv.w, cv.h))
}

pub fn poly(cv: &mut Canvas, pts: &[P], c: Rgba32) -> Mask {
    let m = pmask(cv, pts);
    cv.fill_solid(&m, c);
    m
}

pub fn path(cv: &mut Canvas, p: &Path, c: Rgba32) -> Mask {
    let m = Mask::from_path(p, (cv.w, cv.h));
    cv.fill_solid(&m, c);
    m
}

pub fn rect(cv: &mut Canvas, x: f32, y: f32, w: f32, h: f32, c: Rgba32) -> Mask {
    poly(cv, &[(x, y), (x + w, y), (x + w, y + h), (x, y + h)], c)
}

pub fn ell(cv: &mut Canvas, cx: f32, cy: f32, rx: f32, ry: f32, c: Rgba32) -> Mask {
    path(cv, &Path::ellipse(cx, cy, rx, ry, 0.0), c)
}

/// 画面全体を縦 1 方向のグラデで塗る（y0 より上は c0、y1 より下は c1）。
pub fn vgrad(cv: &mut Canvas, y0: f32, y1: f32, c0: Rgba32, c1: Rgba32) {
    cv.fill_all(|_, y| mix(c0, c1, (y - y0) / (y1 - y0)));
}

/// `m` の内側だけを縦グラデで塗る。
pub fn vgrad_in(cv: &mut Canvas, m: &Mask, y0: f32, y1: f32, c0: Rgba32, c1: Rgba32) {
    cv.fill_mask(m, &|_, y| mix(c0, c1, (y - y0) / (y1 - y0)));
}

/// 太さ一定の線分（丸端）。
pub fn bar(cv: &mut Canvas, a: P, b: P, w: f32, c: Rgba32) {
    let m = Mask::from_path(&Path::stroke(&[(a.0, a.1, w), (b.0, b.1, w)]), (cv.w, cv.h));
    cv.fill_solid(&m, c);
}

/// 5 角ではなく 4 方向の小さな瞬き星（ひし形）。
pub fn sparkle(cv: &mut Canvas, x: f32, y: f32, r: f32, c: Rgba32) {
    poly(
        cv,
        &[
            (x, y - r),
            (x + r * 0.28, y - r * 0.28),
            (x + r, y),
            (x + r * 0.28, y + r * 0.28),
            (x, y + r),
            (x - r * 0.28, y + r * 0.28),
            (x - r, y),
            (x - r * 0.28, y - r * 0.28),
        ],
        c,
    );
}

/// 先細りの流れ星（頭 `h` から尾の向き `(dx, dy)` へ `len`）。尾は淡い帯 + 明るい芯の 2 枚のベタ。
pub fn meteor(cv: &mut Canvas, h: P, dir: P, len: f32, w: f32) {
    let tail = (h.0 + dir.0 * len, h.1 + dir.1 * len);
    let n = (-dir.1, dir.0);
    let band = |cv: &mut Canvas, k: f32, c: Rgba32| {
        let ww = w * k;
        poly(
            cv,
            &[
                (h.0 + n.0 * ww, h.1 + n.1 * ww),
                (tail.0, tail.1),
                (h.0 - n.0 * ww, h.1 - n.1 * ww),
            ],
            c,
        );
    };
    band(cv, 2.2, cola(200, 214, 255, 0.28));
    let core = (h.0 + dir.0 * len * 0.55, h.1 + dir.1 * len * 0.55);
    let ww = w * 0.9;
    poly(
        cv,
        &[
            (h.0 + n.0 * ww, h.1 + n.1 * ww),
            core,
            (h.0 - n.0 * ww, h.1 - n.1 * ww),
        ],
        col(246, 248, 255),
    );
    ell(cv, h.0, h.1, w * 1.3, w * 1.3, col(255, 255, 255));
}
