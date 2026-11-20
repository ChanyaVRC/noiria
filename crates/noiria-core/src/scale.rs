//! 表示用の拡大縮小（BGRX/RGBA の u32 列）。OS の拡大転送（Windows の HALFTONE の `StretchDIBits`）は、等倍でない
//! 表示で 1 フレーム 15〜20ms かかり、メニューのようにフレームごとに描き直す画面を 30fps 以下にしていた。
//! ここは整数演算の双一次補間（2 チャンネルずつ SWAR）を行ごとに分けて std のスレッドで並列に回す。
//! 1920x1080 へ 1 フレーム約 1〜2ms。画素の位置は中心合わせ（`(dx + 0.5) * sw / dw - 0.5`）。

/// 2 画素を重み `w`（0..=256、`b` の割合）で混ぜる。4 チャンネルを 2 つずつ 16 ビット幅に載せて乗算する。
#[inline(always)]
fn lerp_px(a: u32, b: u32, w: u32) -> u32 {
    let iw = 256 - w;
    let rb = (((a & 0x00FF_00FF) * iw + (b & 0x00FF_00FF) * w + 0x0080_0080) >> 8) & 0x00FF_00FF;
    let ga = (((a >> 8) & 0x00FF_00FF) * iw + ((b >> 8) & 0x00FF_00FF) * w + 0x0080_0080) & 0xFF00_FF00;
    rb | ga
}

/// 出力 1 方向ぶんの標本位置（左の画素, 右の画素, 右の重み 0..=256）。
fn axis(dst: usize, src: usize) -> Vec<(u32, u32, u32)> {
    (0..dst)
        .map(|d| {
            let pos = ((d as f64 + 0.5) * src as f64 / dst as f64 - 0.5).clamp(0.0, (src - 1) as f64);
            let i0 = pos.floor() as usize;
            let i1 = (i0 + 1).min(src - 1);
            (i0 as u32, i1 as u32, ((pos - i0 as f64) * 256.0).round() as u32)
        })
        .collect()
}

fn scale_rows(src: &[u32], sw: usize, xs: &[(u32, u32, u32)], ys: &[(u32, u32, u32)], y_start: usize, dst: &mut [u32]) {
    let dw = xs.len();
    for (r, row) in dst.chunks_exact_mut(dw).enumerate() {
        let (y0, y1, wy) = ys[y_start + r];
        let (r0, r1) = (&src[y0 as usize * sw..][..sw], &src[y1 as usize * sw..][..sw]);
        for (o, &(x0, x1, wx)) in row.iter_mut().zip(xs.iter()) {
            let top = lerp_px(r0[x0 as usize], r0[x1 as usize], wx);
            let bot = lerp_px(r1[x0 as usize], r1[x1 as usize], wx);
            *o = lerp_px(top, bot, wy);
        }
    }
}

/// `src`（`sw`x`sh`）を `dst`（`dw`x`dh`）へ双一次補間で拡縮する。`threads` > 1 なら行を分けて並列に回す。
pub fn scale_bilinear(src: &[u32], sw: usize, sh: usize, dst: &mut [u32], dw: usize, dh: usize, threads: usize) {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.len() < sw * sh || dst.len() < dw * dh {
        return;
    }
    if sw == dw && sh == dh {
        dst[..dw * dh].copy_from_slice(&src[..sw * sh]);
        return;
    }
    let xs = axis(dw, sw);
    let ys = axis(dh, sh);
    let threads = threads.clamp(1, 16).min(dh);
    if threads == 1 {
        scale_rows(src, sw, &xs, &ys, 0, &mut dst[..dw * dh]);
        return;
    }
    let band = dh.div_ceil(threads);
    std::thread::scope(|s| {
        for (i, chunk) in dst[..dw * dh].chunks_mut(band * dw).enumerate() {
            let (xs, ys) = (&xs, &ys);
            s.spawn(move || scale_rows(src, sw, xs, ys, i * band, chunk));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(r: u32, g: u32, b: u32, a: u32) -> u32 {
        (a << 24) | (r << 16) | (g << 8) | b
    }

    fn naive(src: &[u32], sw: usize, sh: usize, dw: usize, dh: usize, x: usize, y: usize) -> [f64; 4] {
        let fx = ((x as f64 + 0.5) * sw as f64 / dw as f64 - 0.5).clamp(0.0, (sw - 1) as f64);
        let fy = ((y as f64 + 0.5) * sh as f64 / dh as f64 - 0.5).clamp(0.0, (sh - 1) as f64);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
        let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
        let mut out = [0.0; 4];
        for (c, o) in out.iter_mut().enumerate() {
            let ch = |p: u32| ((p >> (8 * c)) & 255) as f64;
            let top = ch(src[y0 * sw + x0]) * (1.0 - tx) + ch(src[y0 * sw + x1]) * tx;
            let bot = ch(src[y1 * sw + x0]) * (1.0 - tx) + ch(src[y1 * sw + x1]) * tx;
            *o = top * (1.0 - ty) + bot * ty;
        }
        out
    }

    #[test]
    fn same_size_is_a_copy_and_a_flat_colour_stays_flat() {
        let src: Vec<u32> = (0..12).map(|i| pack(i, 255 - i, i * 7, 255)).collect();
        let mut dst = vec![0; 12];
        scale_bilinear(&src, 4, 3, &mut dst, 4, 3, 2);
        assert_eq!(src, dst);
        let flat = vec![pack(10, 200, 99, 255); 20];
        let mut big = vec![0; 7 * 9];
        scale_bilinear(&flat, 5, 4, &mut big, 7, 9, 3);
        assert!(big.iter().all(|&p| p == pack(10, 200, 99, 255)));
    }

    #[test]
    fn matches_a_float_bilinear_within_a_level_and_a_half_and_is_thread_count_independent() {
        let (sw, sh, dw, dh) = (16usize, 9usize, 24usize, 14usize);
        let src: Vec<u32> = (0..sw * sh)
            .map(|i| pack((i * 37 % 256) as u32, (i * 91 % 256) as u32, (i * 13 % 256) as u32, 255))
            .collect();
        let mut one = vec![0; dw * dh];
        let mut four = vec![0; dw * dh];
        scale_bilinear(&src, sw, sh, &mut one, dw, dh, 1);
        scale_bilinear(&src, sw, sh, &mut four, dw, dh, 4);
        assert_eq!(one, four, "行の分け方で結果が変わらない");
        for y in 0..dh {
            for x in 0..dw {
                let want = naive(&src, sw, sh, dw, dh, x, y);
                let got = one[y * dw + x];
                for (c, w) in want.iter().enumerate() {
                    let g = ((got >> (8 * c)) & 255) as f64;
                    assert!((g - w).abs() <= 1.5, "({x},{y}) ch{c}: {g} vs {w}");
                }
            }
        }
    }

    /// `cargo test --release -p noiria-core scale_cost -- --ignored --nocapture`: 拡大先ごとの所要時間（ms）。
    #[test]
    #[ignore]
    fn scale_cost() {
        let (sw, sh) = (1280usize, 720usize);
        let src: Vec<u32> = (0..sw * sh).map(|i| (i as u32).wrapping_mul(2654435761)).collect();
        for (dw, dh) in [(1920usize, 1080usize), (2560, 1440), (3840, 2160)] {
            let mut dst = vec![0u32; dw * dh];
            for threads in [1usize, 4] {
                let t = std::time::Instant::now();
                for _ in 0..10 {
                    scale_bilinear(&src, sw, sh, &mut dst, dw, dh, threads);
                }
                println!("{dw}x{dh} threads={threads}: {:.2} ms", t.elapsed().as_secs_f64() * 100.0);
            }
        }
    }

    /// 目視確認用: `SCALE_IN=<png> SCALE_OUT=<png> SCALE_W=1920 SCALE_H=1080 cargo test -p noiria-core scale_png -- --ignored`
    #[test]
    #[ignore]
    fn scale_png() {
        let (Ok(i), Ok(o)) = (std::env::var("SCALE_IN"), std::env::var("SCALE_OUT")) else {
            return;
        };
        let (dw, dh): (usize, usize) = (
            std::env::var("SCALE_W").ok().and_then(|v| v.parse().ok()).unwrap_or(1920),
            std::env::var("SCALE_H").ok().and_then(|v| v.parse().ok()).unwrap_or(1080),
        );
        let png = crate::codec::png::decode(&std::fs::read(i).unwrap()).unwrap();
        let src: Vec<u32> = png.rgba.as_chunks::<4>().0.iter().map(|p| pack(p[0] as u32, p[1] as u32, p[2] as u32, 255)).collect();
        let mut dst = vec![0u32; dw * dh];
        scale_bilinear(&src, png.width as usize, png.height as usize, &mut dst, dw, dh, 4);
        let mut out = crate::pixmap::Pixmap::new(dw as u32, dh as u32);
        for (px, d) in dst.iter().zip(out.data.as_chunks_mut::<4>().0) {
            d.copy_from_slice(&[(px >> 16) as u8, (px >> 8) as u8, *px as u8, 255]);
        }
        crate::assets::save_png(&out, std::path::Path::new(&o)).unwrap();
    }
}
