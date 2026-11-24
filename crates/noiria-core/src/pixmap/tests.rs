use super::*;

#[test]
fn checked_dims_rejects_oversized_and_wrapping_dimensions() {
    assert_eq!(checked_dims(1280, 720), Ok(1280 * 720 * 4));
    assert_eq!(checked_dims(0, 5), Ok(0));
    assert_eq!(checked_dims(MAX_DIM, 4096), Ok(MAX_DIM as usize * 4096 * 4));
    for (w, h) in [
        (100_000, 100_000),
        (1 << 31, 1 << 31),
        (u32::MAX, 1),
        (MAX_DIM + 1, 1),
        (MAX_DIM, MAX_DIM),
    ] {
        assert!(checked_dims(w, h).is_err(), "{w}x{h}");
        assert!(Pixmap::try_new(w, h).is_err(), "{w}x{h}");
    }
    let p = Pixmap::try_new(3, 2).unwrap();
    assert_eq!((p.width, p.height, p.data.len()), (3, 2, 24));
}

#[test]
fn resize_area_averages_and_preserves_flat_color() {
    // 単色はどの比率でも単色のまま（重みの正規化が崩れていない）。
    let mut flat = Pixmap::new(1280, 720);
    flat.fill([10, 20, 30, 255]);
    let small = flat.resize_area(240, 135);
    assert_eq!((small.width, small.height), (240, 135));
    assert!(small
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| *p == [10, 20, 30, 255]));

    // 左半分が黒・右半分が白の 4x1 を 2x1 へ: 各画素は元の 2 画素の平均。
    let mut p = Pixmap::new(4, 1);
    p.data = vec![
        0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    ];
    assert_eq!(
        p.resize_area(2, 1).data,
        vec![0, 0, 0, 255, 255, 255, 255, 255]
    );
    // 4x1 を 3x1 へ（非整数比）: 中央の画素は黒と白を半分ずつ。
    let mid = &p.resize_area(3, 1).data[4..8];
    assert!((126..=129).contains(&mid[0]) && mid[3] == 255, "{mid:?}");
}

#[test]
fn resize_area_degenerate_sizes_are_empty() {
    let p = Pixmap::new(4, 4);
    assert_eq!(p.resize_area(0, 3).data.len(), 0);
    assert_eq!(Pixmap::new(0, 0).resize_area(2, 2).data, vec![0; 16]);
}

#[test]
fn premultiply_on_construct() {
    // 半透明赤 (255,0,0,128) → premul (128,0,0,128)
    let p = Pixmap::from_rgba_straight(1, 1, &[255, 0, 0, 128]);
    assert_eq!(&p.data, &[128, 0, 0, 128]);
}

#[test]
fn from_rgba_opaque_reuses_buffer_and_matches_straight() {
    let raw = vec![10, 20, 30, 255, 200, 150, 100, 255];
    let expected = Pixmap::from_rgba_straight(2, 1, &raw);
    let ptr = raw.as_ptr();
    let opaque = Pixmap::from_rgba_opaque(2, 1, raw);
    assert_eq!(opaque, expected);
    assert_eq!(opaque.data.as_ptr(), ptr);
}

#[test]
fn over_composite() {
    let mut dst = Pixmap::new(1, 1);
    dst.fill([0, 0, 255, 255]); // 青、不透明
    let src = Pixmap::from_rgba_straight(1, 1, &[255, 0, 0, 128]); // 半透明赤
    dst.blit_over(&src, 0, 0, 255);
    // out = src + dst*(1-128/255) ≈ (128, 0, 127, 255)
    assert_eq!(dst.data[0], 128);
    assert_eq!(dst.data[1], 0);
    assert!((dst.data[2] as i32 - 127).abs() <= 1, "b={}", dst.data[2]);
    assert_eq!(dst.data[3], 255);
}

#[test]
fn lerp_midpoint() {
    let mut a = Pixmap::new(1, 1);
    a.fill([0, 0, 0, 255]);
    let mut b = Pixmap::new(1, 1);
    b.fill([255, 255, 255, 255]);
    let mut out = Pixmap::new(1, 1);
    out.lerp_from(&a, &b, 0.5);
    assert!((out.data[0] as i32 - 127).abs() <= 1);
}

#[test]
fn universal_rule_edges() {
    let mut a = Pixmap::new(2, 1);
    a.fill([0, 0, 0, 255]);
    let mut b = Pixmap::new(2, 1);
    b.fill([255, 255, 255, 255]);
    let mut out = Pixmap::new(2, 1);
    // rule: 左=0 (先に切替), 右=255 (最後に切替)
    let rule = [0u8, 255u8];
    out.universal_from(&a, &b, &rule, 0.5, 1.0);
    // 左ピクセルは完全に b、右ピクセルはまだ a
    assert!(out.data[0] > 250, "left should be b: {}", out.data[0]);
    assert!(out.data[4] < 5, "right should be a: {}", out.data[4]);
}

#[test]
fn extract_region_copies_subrect() {
    // 3 マス (各 2x2px) を横に並べた 6x2 の画像から中央のマスだけ切り出す
    // （[button] の 3 状態画像スプライトを想定）。
    let mut src = Pixmap::new(6, 2);
    src.fill([0, 0, 0, 0]);
    // 中央マス (x=2..4) を赤で塗る
    let red = Pixmap::from_rgba_straight(2, 2, &[255, 0, 0, 255].repeat(4));
    src.blit_over(&red, 2, 0, 255);
    let region = src.extract_region(2, 0, 2, 2);
    assert_eq!((region.width, region.height), (2, 2));
    for px in region.data.as_chunks::<4>().0 {
        assert_eq!(px, &[255, 0, 0, 255]);
    }
}

#[test]
fn extract_region_out_of_bounds_is_transparent_not_panic() {
    let src = Pixmap::new(4, 4);
    let region = src.extract_region(2, 2, 8, 8);
    assert_eq!((region.width, region.height), (8, 8));
    assert!(region.data.iter().all(|&b| b == 0));
}

/// flip_h/flip_v（W5）用の非対称フィクスチャ: 2x2 の 4 隅にそれぞれ違う色を置く。
/// 左右対称・上下対称なフィクスチャだと反転軸の取り違え（h と v の実装を入れ替える等の
/// バグ）を検出できない教訓（MF 動画で対称フィクスチャのせいで反転バグを見逃した経緯、
/// `docs/implementation-notes.md`「2026-07-11 W5」参照）を踏まえ、4 隅すべて非対称にする。
fn corner_fixture() -> Pixmap {
    let mut p = Pixmap::new(2, 2);
    p.data[0..4].copy_from_slice(&[255, 0, 0, 255]); // (0,0) 左上 = 赤
    p.data[4..8].copy_from_slice(&[0, 255, 0, 255]); // (1,0) 右上 = 緑
    p.data[8..12].copy_from_slice(&[0, 0, 255, 255]); // (0,1) 左下 = 青
    p.data[12..16].copy_from_slice(&[255, 255, 0, 255]); // (1,1) 右下 = 黄
    p
}

fn px_at(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * p.width + x) * 4) as usize;
    [p.data[i], p.data[i + 1], p.data[i + 2], p.data[i + 3]]
}

#[test]
fn flip_h_mirrors_left_right_not_up_down() {
    let out = corner_fixture().flip_h();
    assert_eq!(
        px_at(&out, 0, 0),
        [0, 255, 0, 255],
        "左上は元・右上(緑)のはず"
    );
    assert_eq!(
        px_at(&out, 1, 0),
        [255, 0, 0, 255],
        "右上は元・左上(赤)のはず"
    );
    assert_eq!(
        px_at(&out, 0, 1),
        [255, 255, 0, 255],
        "左下は元・右下(黄)のはず"
    );
    assert_eq!(
        px_at(&out, 1, 1),
        [0, 0, 255, 255],
        "右下は元・左下(青)のはず（上下は不変）"
    );
}

#[test]
fn flip_v_mirrors_up_down_not_left_right() {
    let out = corner_fixture().flip_v();
    assert_eq!(
        px_at(&out, 0, 0),
        [0, 0, 255, 255],
        "左上は元・左下(青)のはず"
    );
    assert_eq!(
        px_at(&out, 1, 0),
        [255, 255, 0, 255],
        "右上は元・右下(黄)のはず"
    );
    assert_eq!(
        px_at(&out, 0, 1),
        [255, 0, 0, 255],
        "左下は元・左上(赤)のはず（左右は不変）"
    );
    assert_eq!(
        px_at(&out, 1, 1),
        [0, 255, 0, 255],
        "右下は元・右上(緑)のはず"
    );
}

#[test]
fn grayscale_converts_to_luma_and_preserves_alpha() {
    let mut p = Pixmap::new(1, 1);
    p.data.copy_from_slice(&[255, 0, 0, 200]); // 純赤、alpha 200
    let out = p.grayscale();
    let y = ((255u32 * 77) >> 8) as u8;
    assert_eq!(
        out.data,
        vec![y, y, y, 200],
        "R/G/B が輝度値に揃い、A は不変のはず"
    );
}

#[test]
fn scroll_from_left_midpoint_shows_new_entering_from_left_edge() {
    let mut old = Pixmap::new(4, 1);
    old.fill([255, 0, 0, 255]); // 赤 = 旧ページ
    let mut new = Pixmap::new(4, 1);
    new.fill([0, 0, 255, 255]); // 青 = 新ページ
    let mut out = Pixmap::new(4, 1);
    out.scroll_from(&old, &new, ScrollDir::Left, 0.5);
    // t=0.5: 新ページは x=-2 から入ってきて左半分 (列0,1) を覆う。右半分は旧ページのまま。
    assert_eq!(px_at(&out, 0, 0), [0, 0, 255, 255]);
    assert_eq!(px_at(&out, 1, 0), [0, 0, 255, 255]);
    assert_eq!(px_at(&out, 2, 0), [255, 0, 0, 255]);
    assert_eq!(px_at(&out, 3, 0), [255, 0, 0, 255]);
}

#[test]
fn scroll_from_right_midpoint_shows_new_entering_from_right_edge() {
    let mut old = Pixmap::new(4, 1);
    old.fill([255, 0, 0, 255]);
    let mut new = Pixmap::new(4, 1);
    new.fill([0, 0, 255, 255]);
    let mut out = Pixmap::new(4, 1);
    out.scroll_from(&old, &new, ScrollDir::Right, 0.5);
    // 左右逆: 新ページは右から入ってきて右半分 (列2,3) を覆う。
    assert_eq!(px_at(&out, 0, 0), [255, 0, 0, 255]);
    assert_eq!(px_at(&out, 1, 0), [255, 0, 0, 255]);
    assert_eq!(px_at(&out, 2, 0), [0, 0, 255, 255]);
    assert_eq!(px_at(&out, 3, 0), [0, 0, 255, 255]);
}

#[test]
fn scroll_from_edges_are_fully_old_then_fully_new() {
    let mut old = Pixmap::new(2, 1);
    old.fill([255, 0, 0, 255]);
    let mut new = Pixmap::new(2, 1);
    new.fill([0, 0, 255, 255]);
    let mut out = Pixmap::new(2, 1);
    out.scroll_from(&old, &new, ScrollDir::Top, 0.0);
    assert_eq!(
        px_at(&out, 0, 0),
        [255, 0, 0, 255],
        "t=0 は完全に旧ページのはず"
    );
    out.scroll_from(&old, &new, ScrollDir::Top, 1.0);
    assert_eq!(
        px_at(&out, 0, 0),
        [0, 0, 255, 255],
        "t=1 は完全に新ページのはず"
    );
}

#[test]
fn blit_clipping() {
    let mut dst = Pixmap::new(4, 4);
    let mut src = Pixmap::new(4, 4);
    src.fill([255, 255, 255, 255]);
    // はみ出しても panic しない
    dst.blit_over(&src, -2, -2, 255);
    dst.blit_over(&src, 3, 3, 255);
    dst.blit_over(&src, 10, 10, 255);
    assert_eq!(dst.data[0], 255); // (-2,-2) blit が (0,0) を塗った
}

fn lcg_bytes(seed: &mut u64, len: usize) -> Vec<u8> {
    (0..len)
        .map(|_| {
            *seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (*seed >> 33) as u8
        })
        .collect()
}

#[test]
fn div255_fast_formulas_match_exact_division_exhaustively() {
    for v in 0..=(255u32 * 255) {
        let exact = div255(v);
        let t = v + 128;
        let shift = (t + (t >> 8)) >> 8;
        let mulhi = (t * 257) >> 16;
        assert_eq!(shift, exact, "shift mismatch at v={v}");
        assert_eq!(mulhi, exact, "mulhi mismatch at v={v}");
    }
}

#[test]
fn to_bgrx_u32_simd_matches_scalar_across_lengths() {
    let mut seed = 1u64;
    for pixels in 0..=35 {
        let data = lcg_bytes(&mut seed, pixels * 4);
        let pm = Pixmap {
            width: pixels as u32,
            height: 1,
            data: data.clone(),
        };
        let mut want = vec![0xDEAD_BEEFu32; pixels + 3];
        to_bgrx_u32_scalar(&data, &mut want[..pixels]);

        let mut got = vec![0xDEAD_BEEFu32; pixels + 3];
        pm.to_bgrx_u32(&mut got[..pixels]);
        assert_eq!(got, want, "public to_bgrx_u32 pixels={pixels}");

        // When out is longer than src, trailing elements remain untouched.
        let mut want_long = vec![0xCAFE_BABEu32; pixels + 5];
        to_bgrx_u32_scalar(&data, &mut want_long);
        let mut got_long = vec![0xCAFE_BABEu32; pixels + 5];
        pm.to_bgrx_u32(&mut got_long);
        assert_eq!(got_long, want_long, "long out pixels={pixels}");

        // When out is shorter than src, only out.len() pixels are written.
        if pixels >= 3 {
            let short = pixels - 2;
            let mut want_short = vec![0u32; short];
            to_bgrx_u32_scalar(&data, &mut want_short);
            let mut got_short = vec![0u32; short];
            pm.to_bgrx_u32(&mut got_short);
            assert_eq!(got_short, want_short, "short out pixels={pixels}");
        }

        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                let mut got_avx2 = vec![0xDEAD_BEEFu32; pixels + 3];
                // SAFETY: AVX2 was checked above.
                unsafe { simd::to_bgrx_u32_avx2(&data, &mut got_avx2[..pixels]) };
                assert_eq!(got_avx2, want, "avx2 pixels={pixels}");
            }
            if is_x86_feature_detected!("ssse3") {
                let mut got_ssse3 = vec![0xDEAD_BEEFu32; pixels + 3];
                // SAFETY: SSSE3 was checked above.
                unsafe { simd::to_bgrx_u32_ssse3(&data, &mut got_ssse3[..pixels]) };
                assert_eq!(got_ssse3, want, "ssse3 pixels={pixels}");
            }
        }
    }
}

#[test]
fn lerp_from_simd_matches_scalar_across_t_edges_and_lengths() {
    let mut seed = 42u64;
    let t_values = [
        -0.5f32,
        0.0,
        1.0 / 256.0,
        0.25,
        127.0 / 256.0,
        0.5,
        129.0 / 256.0,
        0.75,
        255.0 / 256.0,
        1.0,
        1.5,
        f32::NAN,
    ];
    for len in [0usize, 1, 3, 4, 15, 16, 17, 31, 32, 33, 63, 64, 67] {
        let a_bytes = lcg_bytes(&mut seed, len * 4);
        let b_bytes = lcg_bytes(&mut seed, len * 4);
        let pa = Pixmap {
            width: len as u32,
            height: 1,
            data: a_bytes.clone(),
        };
        let pb = Pixmap {
            width: len as u32,
            height: 1,
            data: b_bytes.clone(),
        };
        for &t in &t_values {
            let t256 = (t.clamp(0.0, 1.0) * 256.0) as u32;
            let mut want = vec![0u8; len * 4];
            lerp_bytes_scalar(&mut want, &a_bytes, &b_bytes, t256);

            let mut out = Pixmap::new(len as u32, 1);
            out.lerp_from(&pa, &pb, t);
            assert_eq!(out.data, want, "public lerp len={len} t={t}");

            #[cfg(target_arch = "x86_64")]
            {
                if is_x86_feature_detected!("avx2") {
                    let mut got = vec![0u8; len * 4];
                    // SAFETY: AVX2 was checked above.
                    unsafe { simd::lerp_bytes_avx2(&mut got, &a_bytes, &b_bytes, t256) };
                    assert_eq!(got, want, "avx2 lerp len={len} t256={t256}");
                }
                let mut got_sse2 = vec![0u8; len * 4];
                // SAFETY: SSE2 is guaranteed on x86_64.
                unsafe { simd::lerp_bytes_sse2(&mut got_sse2, &a_bytes, &b_bytes, t256) };
                assert_eq!(got_sse2, want, "sse2 lerp len={len} t256={t256}");
            }
        }
    }

    // Exhaustive check over all 256x256 (a, b) byte pairs for key t256 values.
    let mut all_a = Vec::with_capacity(256 * 256);
    let mut all_b = Vec::with_capacity(256 * 256);
    for a in 0..=255u8 {
        for b in 0..=255u8 {
            all_a.push(a);
            all_b.push(b);
        }
    }
    for t256 in [0u32, 1, 64, 127, 128, 191, 255, 256] {
        let mut want = vec![0u8; all_a.len()];
        lerp_bytes_scalar(&mut want, &all_a, &all_b, t256);
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                let mut got = vec![0u8; all_a.len()];
                // SAFETY: AVX2 was checked above.
                unsafe { simd::lerp_bytes_avx2(&mut got, &all_a, &all_b, t256) };
                assert_eq!(got, want, "exhaustive avx2 t256={t256}");
            }
            let mut got_sse2 = vec![0u8; all_a.len()];
            // SAFETY: SSE2 is guaranteed on x86_64.
            unsafe { simd::lerp_bytes_sse2(&mut got_sse2, &all_a, &all_b, t256) };
            assert_eq!(got_sse2, want, "exhaustive sse2 t256={t256}");
        }
    }
}

#[test]
fn from_rgba_straight_simd_matches_scalar() {
    let mut seed = 99u64;
    for pixels in [0usize, 1, 3, 4, 7, 8, 9, 15, 16, 17, 33] {
        let mut rgba = lcg_bytes(&mut seed, pixels * 4);
        // Mix all-opaque and all-transparent runs to exercise fast paths alongside semi-transparent.
        if pixels >= 24 {
            for px in rgba[0..32].as_chunks_mut::<4>().0 {
                px[3] = 255;
            }
            for px in rgba[32..64].as_chunks_mut::<4>().0 {
                px[3] = 0;
            }
        }
        let mut want = vec![0u8; rgba.len()];
        premultiply_rgba_scalar(&rgba, &mut want);

        let got = Pixmap::from_rgba_straight(pixels as u32, 1, &rgba);
        assert_eq!(got.data, want, "public premul pixels={pixels}");
        let got_owned = Pixmap::from_rgba_straight_owned(pixels as u32, 1, rgba.clone());
        assert_eq!(got_owned.data, want, "owned premul pixels={pixels}");

        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                let mut got_avx2 = vec![0u8; rgba.len()];
                // SAFETY: AVX2 was checked above.
                unsafe {
                    simd::premultiply_rgba_avx2_raw(
                        rgba.as_ptr(),
                        got_avx2.as_mut_ptr(),
                        rgba.len(),
                    );
                }
                assert_eq!(got_avx2, want, "avx2 premul pixels={pixels}");
            }
            let mut got_sse2 = vec![0u8; rgba.len()];
            // SAFETY: SSE2 is guaranteed on x86_64.
            unsafe {
                simd::premultiply_rgba_sse2_raw(rgba.as_ptr(), got_sse2.as_mut_ptr(), rgba.len());
            }
            assert_eq!(got_sse2, want, "sse2 premul pixels={pixels}");
        }
    }
}

fn blit_over_clipped_scalar_ref(
    dst: &mut Pixmap,
    src: &Pixmap,
    x: i32,
    y: i32,
    opacity: u8,
    max_w: Option<u32>,
) {
    if opacity == 0 {
        return;
    }
    let o = opacity as u32;
    let dst_w = dst.width as i32;
    let dst_h = dst.height as i32;
    let src_w = src.width.min(max_w.unwrap_or(u32::MAX)) as i32;
    let src_h = src.height as i32;
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + src_w).min(dst_w);
    let y1 = (y + src_h).min(dst_h);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    for ty in y0..y1 {
        let sy = (ty - y) as usize;
        let drow = (ty as usize * dst.width as usize + x0 as usize) * 4;
        let srow = (sy * src.width as usize + (x0 - x) as usize) * 4;
        let row_bytes = ((x1 - x0) as usize) * 4;
        if opacity == 255 {
            blit_row_opaque_scalar(
                &mut dst.data[drow..drow + row_bytes],
                &src.data[srow..srow + row_bytes],
            );
        } else {
            blit_row_opacity_scalar(
                &mut dst.data[drow..drow + row_bytes],
                &src.data[srow..srow + row_bytes],
                o,
            );
        }
    }
}

#[test]
fn blit_over_clipped_simd_matches_scalar_exhaustively() {
    let mut seed = 12345u64;
    let opacities = [0u8, 1, 2, 64, 127, 128, 129, 254, 255];

    // Row-level tests across widths including tails, all-zero, all-opaque, additive (a=0, rgb>0), saturation.
    for width in [0usize, 1, 3, 4, 7, 8, 9, 15, 16, 17, 33, 40] {
        let mut src_row = lcg_bytes(&mut seed, width * 4);
        let dst_orig = lcg_bytes(&mut seed, width * 4);
        if width >= 32 {
            // Pixels 0..8: all zero (transparent fast-skip)
            src_row[0..32].fill(0);
            // Pixels 8..16: all opaque (opaque fast-copy)
            for px in src_row[32..64].as_chunks_mut::<4>().0 {
                px[3] = 255;
            }
            // Pixels 16..24: alpha == 0 with non-zero RGB (additive/saturation edge)
            for px in src_row[64..96].as_chunks_mut::<4>().0 {
                px[0] = 250;
                px[1] = 200;
                px[2] = 128;
                px[3] = 0;
            }
            // Pixels 24..32: extreme channel values
            for (k, px) in src_row[96..128]
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .enumerate()
            {
                let v = [0u8, 1, 127, 128, 254, 255, 255, 1][k];
                *px = [v, 255 - v, v, [0, 1, 127, 128, 254, 255, 1, 254][k]];
            }
        }

        for &op in &opacities {
            if op == 0 {
                continue;
            }
            let mut want = dst_orig.clone();
            if op == 255 {
                blit_row_opaque_scalar(&mut want, &src_row);
            } else {
                blit_row_opacity_scalar(&mut want, &src_row, op as u32);
            }

            #[cfg(target_arch = "x86_64")]
            {
                if is_x86_feature_detected!("avx2") {
                    let mut got = dst_orig.clone();
                    // SAFETY: AVX2 was checked above.
                    unsafe {
                        if op == 255 {
                            simd::blit_row_opaque_avx2(&mut got, &src_row);
                        } else {
                            simd::blit_row_opacity_avx2(&mut got, &src_row, op as u32);
                        }
                    }
                    assert_eq!(got, want, "avx2 blit row width={width} op={op}");
                }
                let mut got_sse2 = dst_orig.clone();
                // SAFETY: SSE2 is guaranteed on x86_64.
                unsafe {
                    if op == 255 {
                        simd::blit_row_opaque_sse2(&mut got_sse2, &src_row);
                    } else {
                        simd::blit_row_opacity_sse2(&mut got_sse2, &src_row, op as u32);
                    }
                }
                assert_eq!(got_sse2, want, "sse2 blit row width={width} op={op}");
            }
        }
    }

    // Full 2D clipped blit tests with offsets, negative coords, odd dimensions, and max_w.
    let dst_base = Pixmap {
        width: 37,
        height: 11,
        data: lcg_bytes(&mut seed, 37 * 11 * 4),
    };
    let src = Pixmap {
        width: 29,
        height: 9,
        data: lcg_bytes(&mut seed, 29 * 9 * 4),
    };
    for &(x, y, max_w) in &[
        (0, 0, None),
        (-5, -3, None),
        (15, 4, None),
        (3, 1, Some(0)),
        (3, 1, Some(1)),
        (3, 1, Some(7)),
        (3, 1, Some(9)),
        (3, 1, Some(17)),
        (-4, 2, Some(19)),
    ] {
        for &op in &[0u8, 1, 128, 255] {
            let mut got = dst_base.clone();
            let mut want = dst_base.clone();
            got.blit_over_clipped(&src, x, y, op, max_w);
            blit_over_clipped_scalar_ref(&mut want, &src, x, y, op, max_w);
            assert_eq!(
                got.data, want.data,
                "clipped x={x} y={y} op={op} max_w={max_w:?}"
            );
        }
    }
}

#[test]
fn universal_from_simd_matches_original_scalar() {
    let mut seed = 777u64;
    for pixels in [0usize, 1, 7, 8, 9, 17, 33, 257] {
        let a = Pixmap {
            width: pixels as u32,
            height: 1,
            data: lcg_bytes(&mut seed, pixels * 4),
        };
        let b = Pixmap {
            width: pixels as u32,
            height: 1,
            data: lcg_bytes(&mut seed, pixels * 4),
        };
        let rule: Vec<u8> = (0..pixels).map(|i| (i % 256) as u8).collect();
        for &(t, vague) in &[
            (0.0f32, 1.0f32),
            (0.25, 16.0),
            (0.5, 64.0),
            (0.75, 255.0),
            (1.0, 0.5),
        ] {
            let mut want = vec![0u8; pixels * 4];
            let v = vague.max(1.0);
            let phase_q8 = (t.clamp(0.0, 1.0) * (255.0 + v) * 256.0) as i64;
            let recip_q16 = (65536.0 / v) as i64;
            for i in 0..pixels {
                let diff_q8 = phase_q8 - ((rule[i] as i64) << 8);
                let t256 = ((diff_q8 * recip_q16) >> 16).clamp(0, 256) as u32;
                let inv = 256 - t256;
                for c in 0..4 {
                    want[i * 4 + c] = ((a.data[i * 4 + c] as u32 * inv
                        + b.data[i * 4 + c] as u32 * t256)
                        >> 8) as u8;
                }
            }
            let mut got = Pixmap::new(pixels as u32, 1);
            got.universal_from(&a, &b, &rule, t, vague);
            assert_eq!(
                got.data, want,
                "universal pixels={pixels} t={t} vague={vague}"
            );
        }
    }
}
