//! 256x256 のアイコン。群青〜紫の角丸バッジに、三日月・金の星・流れ星。

use super::*;
use image::RgbaImage;

fn sdf_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let r = r.min(hw).min(hh).max(0.0);
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

pub fn draw() -> RgbaImage {
    let mut cv = Canvas::new(256, 256);
    let mut rng = Rng::new(256);
    cv.fill_all(|x, y| {
        let t = y / 256.0;
        let base = mix(col(0x16, 0x1d, 0x62), col(0x72, 0x45, 0xbc), t.powf(0.9));
        let d = (((x - 150.0) / 150.0).powi(2) + ((y - 150.0) / 130.0).powi(2)).sqrt();
        mix(
            base,
            col(0xa0, 0x6a, 0xd8),
            (1.0 - d).max(0.0).powi(2) * 0.6,
        )
    });
    // 小さな星
    for _ in 0..46 {
        let (x, y) = (rng.r(20.0, 236.0), rng.r(20.0, 236.0));
        let a = rng.r(0.35, 0.9);
        star_px(&mut cv, x, y, rng.r(0.6, 1.1), [1.0, 0.97, 0.9], a);
    }
    for &(x, y) in &[(60.0f32, 168.0f32), (206.0, 196.0), (112.0, 40.0)] {
        star_px(&mut cv, x, y, 1.2, [1.0, 1.0, 1.0], 0.95);
        flare(&mut cv, x, y, 10.0, [1.0, 0.95, 0.85], 0.7, false);
    }
    // 三日月
    let (mx, my, mr) = (88.0f32, 96.0f32, 36.0f32);
    cv.glow(mx, my, 70.0, [1.0, 0.86, 0.55], 0.22, 2.0);
    let moon =
        Mask::from_path(&Path::ellipse(mx, my, mr, mr, 0.0), (256, 256)).sub(&Mask::from_path(
            &Path::ellipse(mx + 17.0, my - 7.0, mr * 0.86, mr * 0.86, 0.0),
            (256, 256),
        ));
    let rim = moon.dilate(2.0).sub(&moon);
    cv.fill_solid(&rim, cola(0xb0, 0x8a, 0x40, 0.8));
    cv.fill_mask(&moon, &|x, y| {
        let t = ((x - (mx - mr)) / (2.0 * mr) * 0.5 + (y - (my - mr)) / (2.0 * mr) * 0.5)
            .clamp(0.0, 1.0);
        mix(col(0xff, 0xf2, 0xc4), col(0xf2, 0xc2, 0x6a), t)
    });
    // 流れ星
    let (hx, hy) = (170.0f32, 66.0f32);
    for i in 0..90 {
        let t = i as f32 / 90.0;
        let (x, y) = (hx + 70.0 * t, hy - 40.0 * t);
        let f = (1.0 - t).powf(1.8);
        star_px(&mut cv, x, y, 1.0 + 1.6 * f, [0.85, 0.92, 1.0], 0.7 * f);
    }
    star_px(&mut cv, hx, hy, 2.4, [1.0, 1.0, 1.0], 1.0);
    star_px(&mut cv, hx, hy, 9.0, [0.9, 0.95, 1.0], 0.3);
    // 金の星
    let (sx, sy, sr) = (158.0f32, 156.0f32, 64.0f32);
    cv.glow(sx, sy, 112.0, [1.0, 0.8, 0.35], 0.42, 2.0);
    let pts = star_pts(sx, sy, sr, sr * 0.46, 0.1, 5);
    let sm = Mask::from_path(&Path::polygon(&pts), (256, 256))
        .blur(1.6)
        .scaled(1.0);
    let sm = sm.mul(&Mask::from_path(
        &Path::ellipse(sx, sy, sr * 1.4, sr * 1.4, 0.0),
        (256, 256),
    ));
    // 角を少し丸めるため、膨張→収縮の代わりにぼかして閾値をかけたものを使う
    let sm = Mask {
        a: sm.a.iter().map(|&v| smooth((v - 0.25) / 0.5)).collect(),
        ..sm
    };
    let ring = sm.dilate(3.0).sub(&sm);
    cv.fill_solid(&ring, col(0x9a, 0x5c, 0x14));
    cv.fill_mask(&sm, &|x, y| {
        let t = ((y - (sy - sr)) / (sr * 1.8)).clamp(0.0, 1.0);
        let c = mix(col(0xff, 0xf0, 0x9a), col(0xf5, 0xa8, 0x1e), t);
        let d = (((x - sx) / sr).powi(2) + ((y - sy) / sr).powi(2)).sqrt();
        mix(c, col(0xff, 0xfb, 0xe0), (1.0 - d * 1.5).max(0.0))
    });
    // 星のファセット（右下を少し暗く）
    let shade = Mask::from_path(
        &Path::polygon(&[
            (sx, sy),
            (sx + 80.0, sy - 4.0),
            (sx + 80.0, sy + 90.0),
            (sx - 20.0, sy + 90.0),
        ]),
        (256, 256),
    )
    .blur(3.0)
    .mul(&sm);
    cv.fill_solid(&shade, cola(0xd0, 0x7a, 0x10, 0.28));
    let hl = Mask::from_path(
        &Path::ellipse(sx - 14.0, sy - 22.0, 12.0, 6.0, -0.9),
        (256, 256),
    )
    .blur(2.0)
    .mul(&sm);
    cv.fill_solid(&hl, cola(255, 255, 255, 0.55));
    // バッジのアルファ
    let (cx, cy, hw, hh) = (128.0f32, 128.0f32, 112.0f32, 112.0f32);
    for y in 0..256usize {
        for x in 0..256usize {
            let d = sdf_round_rect(x as f32 + 0.5, y as f32 + 0.5, cx, cy, hw, hh, 48.0);
            let cov = (0.5 - d).clamp(0.0, 1.0);
            // 縁のわずかな内側ビネットと上辺ハイライト
            let edge = (-d / 18.0).clamp(0.0, 1.0);
            let k = 0.78 + 0.22 * smooth(edge);
            let top = (1.0 - ((y as f32 - 16.0) / 40.0).clamp(0.0, 1.0))
                * (1.0 - (-d / 6.0).clamp(0.0, 1.0))
                * 0.12;
            let p = &mut cv.px[y * 256 + x];
            for v in p.iter_mut().take(3) {
                *v = (*v * k + top) * cov;
            }
            p[3] = cov;
        }
    }
    cv.to_image(false)
}
