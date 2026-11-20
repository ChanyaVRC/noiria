//! トランジション用ルール画像。星形に散って開く（輝度が低い所から先に開く）。

use super::*;
use image::{Rgba, RgbaImage};

struct Star {
    x: f32,
    y: f32,
    r: f32,
    v0: f32,
    pts: Vec<P>,
}

fn seg_dist(p: P, a: P, b: P) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = (dx * dx + dy * dy).max(1e-9);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0);
    let (qx, qy) = (a.0 + dx * t, a.1 + dy * t);
    ((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt()
}

fn inside(p: P, poly: &[P]) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            c = !c;
        }
    }
    c
}

pub fn draw() -> RgbaImage {
    let mut rng = Rng::new(4242);
    let mut stars: Vec<Star> = Vec::new();
    let sizes: Vec<f32> = {
        let mut v: Vec<f32> = (0..140).map(|_| rng.r(0.0, 1.0)).map(|t| 22.0 + 52.0 * t * t).collect();
        v.sort_by(|a, b| b.partial_cmp(a).unwrap());
        v
    };
    for &r in &sizes {
        if stars.len() >= 86 {
            break;
        }
        for _ in 0..400 {
            let (x, y) = (rng.r(-10.0, 1290.0), rng.r(-10.0, 730.0));
            let ok = stars.iter().all(|s| {
                let d = ((s.x - x).powi(2) + (s.y - y).powi(2)).sqrt();
                d > (s.r + r) * 0.92 + 14.0
            });
            if ok {
                let rot = rng.r(-0.6, 0.6);
                stars.push(Star { x, y, r, v0: rng.r(30.0, 180.0), pts: star_pts(x, y, r, r * 0.42, rot, 5) });
                break;
            }
        }
    }
    let mut lum = vec![255.0f32; W * H];
    for s in &stars {
        let ext = s.r * 1.6;
        let inr = s.r * 0.36;
        let x0 = ((s.x - ext).floor().max(0.0)) as usize;
        let x1 = ((s.x + ext).ceil().min(W as f32)) as usize;
        let y0 = ((s.y - ext).floor().max(0.0)) as usize;
        let y1 = ((s.y + ext).ceil().min(H as f32)) as usize;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = (x as f32 + 0.5, y as f32 + 0.5);
                let mut d = f32::MAX;
                for i in 0..s.pts.len() {
                    d = d.min(seg_dist(p, s.pts[i], s.pts[(i + 1) % s.pts.len()]));
                }
                let sd = if inside(p, &s.pts) { -d } else { d };
                // 中心の寄り（ほぼ円錐）と輪郭からの距離を合成: 中心は暗く、縁へ向かって明るく
                let rc = ((p.0 - s.x).powi(2) + (p.1 - s.y).powi(2)).sqrt() / s.r;
                let f_c = smooth(rc / 1.25).powf(0.85);
                let f_e = smooth((sd + inr) / (inr + s.r * 0.22));
                let f = (f_c * 0.4 + f_e * 0.6).clamp(0.0, 1.0);
                let v = lerpf(s.v0, 255.0, f);
                let idx = y * W + x;
                if v < lum[idx] {
                    lum[idx] = v;
                }
            }
        }
    }
    let mut img = RgbaImage::new(W as u32, H as u32);
    for y in 0..H {
        for x in 0..W {
            let v = lum[y * W + x].round().clamp(0.0, 255.0) as u8;
            img.put_pixel(x as u32, y as u32, Rgba([v, v, v, 255]));
        }
    }
    img
}
