//! x86_64 AVX2/SSE2/SSSE3 pixel kernels for `Pixmap`.

use std::arch::x86_64::*;

use super::{
    blit_row_opacity_scalar, blit_row_opaque_scalar, lerp_bytes_scalar, premultiply_rgba_scalar,
    to_bgrx_u32_scalar, universal_lut_scalar,
};

// For 0 <= v <= 65025, ((v + 128) * 257) >> 16 == ((v + 128) + ((v + 128) >> 8)) >> 8 == (v + 127) / 255.
// v + 128 <= 65153 fits in u16 without overflow, so vpmulhuw computes exact div255 in 1 instruction.
/// # Safety
/// Caller must verify `is_x86_feature_detected!("avx2")`.
#[target_feature(enable = "avx2")]
#[inline]
unsafe fn div255_epi16_avx2(v: __m256i, v128: __m256i, v0101: __m256i) -> __m256i {
    _mm256_mulhi_epu16(_mm256_add_epi16(v, v128), v0101)
}

/// # Safety
/// Caller must ensure SSE2 is available (guaranteed on x86_64).
#[target_feature(enable = "sse2")]
#[inline]
unsafe fn div255_epi16_sse2(v: __m128i, v128: __m128i, v0101: __m128i) -> __m128i {
    _mm_mulhi_epu16(_mm_add_epi16(v, v128), v0101)
}

/// # Safety
/// Caller must verify `is_x86_feature_detected!("avx2")`.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn to_bgrx_u32_avx2(src: &[u8], out: &mut [u32]) {
    let n = out.len().min(src.len() / 4);
    let mask = _mm256_setr_epi8(
        2, 1, 0, -128, 6, 5, 4, -128, 10, 9, 8, -128, 14, 13, 12, -128,
        2, 1, 0, -128, 6, 5, 4, -128, 10, 9, 8, -128, 14, 13, 12, -128,
    );
    let mut i = 0;
    let src_ptr = src.as_ptr();
    let dst_ptr = out.as_mut_ptr();
    while i + 8 <= n {
        let v = _mm256_loadu_si256(src_ptr.add(i * 4) as *const __m256i);
        let shuffled = _mm256_shuffle_epi8(v, mask);
        _mm256_storeu_si256(dst_ptr.add(i) as *mut __m256i, shuffled);
        i += 8;
    }
    to_bgrx_u32_scalar(&src[i * 4..], &mut out[i..]);
}

/// # Safety
/// Caller must verify `is_x86_feature_detected!("ssse3")`.
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn to_bgrx_u32_ssse3(src: &[u8], out: &mut [u32]) {
    let n = out.len().min(src.len() / 4);
    let mask = _mm_setr_epi8(2, 1, 0, -128, 6, 5, 4, -128, 10, 9, 8, -128, 14, 13, 12, -128);
    let mut i = 0;
    let src_ptr = src.as_ptr();
    let dst_ptr = out.as_mut_ptr();
    while i + 4 <= n {
        let v = _mm_loadu_si128(src_ptr.add(i * 4) as *const __m128i);
        let shuffled = _mm_shuffle_epi8(v, mask);
        _mm_storeu_si128(dst_ptr.add(i) as *mut __m128i, shuffled);
        i += 4;
    }
    to_bgrx_u32_scalar(&src[i * 4..], &mut out[i..]);
}

/// # Safety
/// Caller must verify `is_x86_feature_detected!("avx2")`.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn lerp_bytes_avx2(out: &mut [u8], a: &[u8], b: &[u8], t256: u32) {
    let len = out.len().min(a.len()).min(b.len());
    let zero = _mm256_setzero_si256();
    let v_t = _mm256_set1_epi16(t256 as i16);
    let v_inv = _mm256_set1_epi16((256 - t256) as i16);
    let mut i = 0;
    let out_ptr = out.as_mut_ptr();
    let a_ptr = a.as_ptr();
    let b_ptr = b.as_ptr();
    while i + 32 <= len {
        let va = _mm256_loadu_si256(a_ptr.add(i) as *const __m256i);
        let vb = _mm256_loadu_si256(b_ptr.add(i) as *const __m256i);
        let a_lo = _mm256_unpacklo_epi8(va, zero);
        let a_hi = _mm256_unpackhi_epi8(va, zero);
        let b_lo = _mm256_unpacklo_epi8(vb, zero);
        let b_hi = _mm256_unpackhi_epi8(vb, zero);
        let lo = _mm256_srli_epi16::<8>(_mm256_add_epi16(
            _mm256_mullo_epi16(a_lo, v_inv),
            _mm256_mullo_epi16(b_lo, v_t),
        ));
        let hi = _mm256_srli_epi16::<8>(_mm256_add_epi16(
            _mm256_mullo_epi16(a_hi, v_inv),
            _mm256_mullo_epi16(b_hi, v_t),
        ));
        _mm256_storeu_si256(out_ptr.add(i) as *mut __m256i, _mm256_packus_epi16(lo, hi));
        i += 32;
    }
    lerp_bytes_scalar(&mut out[i..len], &a[i..len], &b[i..len], t256);
}

/// # Safety
/// Caller must ensure SSE2 is available (guaranteed on x86_64).
#[target_feature(enable = "sse2")]
pub(super) unsafe fn lerp_bytes_sse2(out: &mut [u8], a: &[u8], b: &[u8], t256: u32) {
    let len = out.len().min(a.len()).min(b.len());
    let zero = _mm_setzero_si128();
    let v_t = _mm_set1_epi16(t256 as i16);
    let v_inv = _mm_set1_epi16((256 - t256) as i16);
    let mut i = 0;
    let out_ptr = out.as_mut_ptr();
    let a_ptr = a.as_ptr();
    let b_ptr = b.as_ptr();
    while i + 16 <= len {
        let va = _mm_loadu_si128(a_ptr.add(i) as *const __m128i);
        let vb = _mm_loadu_si128(b_ptr.add(i) as *const __m128i);
        let a_lo = _mm_unpacklo_epi8(va, zero);
        let a_hi = _mm_unpackhi_epi8(va, zero);
        let b_lo = _mm_unpacklo_epi8(vb, zero);
        let b_hi = _mm_unpackhi_epi8(vb, zero);
        let lo = _mm_srli_epi16::<8>(_mm_add_epi16(
            _mm_mullo_epi16(a_lo, v_inv),
            _mm_mullo_epi16(b_lo, v_t),
        ));
        let hi = _mm_srli_epi16::<8>(_mm_add_epi16(
            _mm_mullo_epi16(a_hi, v_inv),
            _mm_mullo_epi16(b_hi, v_t),
        ));
        _mm_storeu_si128(out_ptr.add(i) as *mut __m128i, _mm_packus_epi16(lo, hi));
        i += 16;
    }
    lerp_bytes_scalar(&mut out[i..len], &a[i..len], &b[i..len], t256);
}

/// # Safety
/// Caller must verify `is_x86_feature_detected!("avx2")`.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn premultiply_rgba_avx2(rgba: &[u8], out: &mut [u8]) {
    let len = rgba.len().min(out.len()) & !3;
    let zero = _mm256_setzero_si256();
    let v128 = _mm256_set1_epi16(128);
    let v0101 = _mm256_set1_epi16(0x0101);
    let alpha_mask = _mm256_set1_epi32(0xFF00_0000u32 as i32);
    let shuf_lo = _mm256_setr_epi8(
        3, -128, 3, -128, 3, -128, 3, -128, 7, -128, 7, -128, 7, -128, 7, -128,
        3, -128, 3, -128, 3, -128, 3, -128, 7, -128, 7, -128, 7, -128, 7, -128,
    );
    let shuf_hi = _mm256_setr_epi8(
        11, -128, 11, -128, 11, -128, 11, -128, 15, -128, 15, -128, 15, -128, 15, -128,
        11, -128, 11, -128, 11, -128, 11, -128, 15, -128, 15, -128, 15, -128, 15, -128,
    );
    let mut i = 0;
    let src_ptr = rgba.as_ptr();
    let dst_ptr = out.as_mut_ptr();
    while i + 32 <= len {
        let vs = _mm256_loadu_si256(src_ptr.add(i) as *const __m256i);
        let res = if _mm256_testc_si256(vs, alpha_mask) != 0 {
            vs
        } else if _mm256_testz_si256(vs, alpha_mask) != 0 {
            zero
        } else {
            let a_lo = _mm256_shuffle_epi8(vs, shuf_lo);
            let a_hi = _mm256_shuffle_epi8(vs, shuf_hi);
            let s_lo = _mm256_unpacklo_epi8(vs, zero);
            let s_hi = _mm256_unpackhi_epi8(vs, zero);
            let p_lo = div255_epi16_avx2(_mm256_mullo_epi16(s_lo, a_lo), v128, v0101);
            let p_hi = div255_epi16_avx2(_mm256_mullo_epi16(s_hi, a_hi), v128, v0101);
            let premul = _mm256_packus_epi16(p_lo, p_hi);
            _mm256_or_si256(
                _mm256_andnot_si256(alpha_mask, premul),
                _mm256_and_si256(alpha_mask, vs),
            )
        };
        _mm256_storeu_si256(dst_ptr.add(i) as *mut __m256i, res);
        i += 32;
    }
    premultiply_rgba_scalar(&rgba[i..len], &mut out[i..len]);
}

/// # Safety
/// Caller must ensure SSE2 is available (guaranteed on x86_64).
#[target_feature(enable = "sse2")]
pub(super) unsafe fn premultiply_rgba_sse2(rgba: &[u8], out: &mut [u8]) {
    let len = rgba.len().min(out.len()) & !3;
    let zero = _mm_setzero_si128();
    let v128 = _mm_set1_epi16(128);
    let v0101 = _mm_set1_epi16(0x0101);
    let alpha_mask = _mm_set1_epi32(0xFF00_0000u32 as i32);
    let mut i = 0;
    let src_ptr = rgba.as_ptr();
    let dst_ptr = out.as_mut_ptr();
    while i + 16 <= len {
        let vs = _mm_loadu_si128(src_ptr.add(i) as *const __m128i);
        let s_lo = _mm_unpacklo_epi8(vs, zero);
        let s_hi = _mm_unpackhi_epi8(vs, zero);
        let a_lo = _mm_shufflehi_epi16::<0xFF>(_mm_shufflelo_epi16::<0xFF>(s_lo));
        let a_hi = _mm_shufflehi_epi16::<0xFF>(_mm_shufflelo_epi16::<0xFF>(s_hi));
        let p_lo = div255_epi16_sse2(_mm_mullo_epi16(s_lo, a_lo), v128, v0101);
        let p_hi = div255_epi16_sse2(_mm_mullo_epi16(s_hi, a_hi), v128, v0101);
        let premul = _mm_packus_epi16(p_lo, p_hi);
        let res = _mm_or_si128(
            _mm_andnot_si128(alpha_mask, premul),
            _mm_and_si128(alpha_mask, vs),
        );
        _mm_storeu_si128(dst_ptr.add(i) as *mut __m128i, res);
        i += 16;
    }
    premultiply_rgba_scalar(&rgba[i..len], &mut out[i..len]);
}

#[derive(Clone, Copy)]
pub(super) struct BlitRect {
    pub(super) d_base: usize,
    pub(super) dst_stride: usize,
    pub(super) s_base: usize,
    pub(super) src_stride: usize,
    pub(super) row_bytes: usize,
    pub(super) rows: usize,
    pub(super) opacity: u32,
}

/// # Safety
/// Caller must verify `is_x86_feature_detected!("avx2")`.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn blit_rect_avx2(dst: &mut [u8], src: &[u8], rect: BlitRect) {
    for r in 0..rect.rows {
        let d0 = rect.d_base + r * rect.dst_stride;
        let s0 = rect.s_base + r * rect.src_stride;
        if rect.opacity == 255 {
            blit_row_opaque_avx2(
                &mut dst[d0..d0 + rect.row_bytes],
                &src[s0..s0 + rect.row_bytes],
            );
        } else {
            blit_row_opacity_avx2(
                &mut dst[d0..d0 + rect.row_bytes],
                &src[s0..s0 + rect.row_bytes],
                rect.opacity,
            );
        }
    }
}

/// # Safety
/// Caller must verify `is_x86_feature_detected!("avx2")`.
#[target_feature(enable = "avx2")]
#[inline]
pub(super) unsafe fn blit_row_opaque_avx2(dst_row: &mut [u8], src_row: &[u8]) {
    let len = dst_row.len().min(src_row.len()) & !3;
    let zero = _mm256_setzero_si256();
    let v_ff = _mm256_set1_epi8(-1);
    let alpha_mask = _mm256_set1_epi32(0xFF00_0000u32 as i32);
    let v128 = _mm256_set1_epi16(128);
    let v0101 = _mm256_set1_epi16(0x0101);
    let shuf_lo = _mm256_setr_epi8(
        3, -128, 3, -128, 3, -128, 3, -128, 7, -128, 7, -128, 7, -128, 7, -128,
        3, -128, 3, -128, 3, -128, 3, -128, 7, -128, 7, -128, 7, -128, 7, -128,
    );
    let shuf_hi = _mm256_setr_epi8(
        11, -128, 11, -128, 11, -128, 11, -128, 15, -128, 15, -128, 15, -128, 15, -128,
        11, -128, 11, -128, 11, -128, 11, -128, 15, -128, 15, -128, 15, -128, 15, -128,
    );
    let mut i = 0;
    let dst_ptr = dst_row.as_mut_ptr();
    let src_ptr = src_row.as_ptr();
    while i + 32 <= len {
        let vs = _mm256_loadu_si256(src_ptr.add(i) as *const __m256i);
        if _mm256_testz_si256(vs, vs) != 0 {
            i += 32;
            continue;
        }
        if _mm256_testc_si256(vs, alpha_mask) != 0 {
            _mm256_storeu_si256(dst_ptr.add(i) as *mut __m256i, vs);
            i += 32;
            continue;
        }
        let vd = _mm256_loadu_si256(dst_ptr.add(i) as *const __m256i);
        let v_inv = _mm256_xor_si256(vs, v_ff);
        let inv_lo = _mm256_shuffle_epi8(v_inv, shuf_lo);
        let inv_hi = _mm256_shuffle_epi8(v_inv, shuf_hi);
        let d_lo = _mm256_unpacklo_epi8(vd, zero);
        let d_hi = _mm256_unpackhi_epi8(vd, zero);
        let s_lo = div255_epi16_avx2(_mm256_mullo_epi16(d_lo, inv_lo), v128, v0101);
        let s_hi = div255_epi16_avx2(_mm256_mullo_epi16(d_hi, inv_hi), v128, v0101);
        let scaled = _mm256_packus_epi16(s_lo, s_hi);
        _mm256_storeu_si256(dst_ptr.add(i) as *mut __m256i, _mm256_adds_epu8(vs, scaled));
        i += 32;
    }
    blit_row_opaque_scalar(&mut dst_row[i..len], &src_row[i..len]);
}

/// # Safety
/// Caller must verify `is_x86_feature_detected!("avx2")`.
#[target_feature(enable = "avx2")]
#[inline]
pub(super) unsafe fn blit_row_opacity_avx2(dst_row: &mut [u8], src_row: &[u8], o: u32) {
    let len = dst_row.len().min(src_row.len()) & !3;
    let zero = _mm256_setzero_si256();
    let v_ff = _mm256_set1_epi8(-1);
    let v_o = _mm256_set1_epi16(o as i16);
    let v128 = _mm256_set1_epi16(128);
    let v0101 = _mm256_set1_epi16(0x0101);
    let shuf_lo = _mm256_setr_epi8(
        3, -128, 3, -128, 3, -128, 3, -128, 7, -128, 7, -128, 7, -128, 7, -128,
        3, -128, 3, -128, 3, -128, 3, -128, 7, -128, 7, -128, 7, -128, 7, -128,
    );
    let shuf_hi = _mm256_setr_epi8(
        11, -128, 11, -128, 11, -128, 11, -128, 15, -128, 15, -128, 15, -128, 15, -128,
        11, -128, 11, -128, 11, -128, 11, -128, 15, -128, 15, -128, 15, -128, 15, -128,
    );
    let mut i = 0;
    let dst_ptr = dst_row.as_mut_ptr();
    let src_ptr = src_row.as_ptr();
    while i + 32 <= len {
        let vs = _mm256_loadu_si256(src_ptr.add(i) as *const __m256i);
        if _mm256_testz_si256(vs, vs) != 0 {
            i += 32;
            continue;
        }
        let sc_lo = div255_epi16_avx2(
            _mm256_mullo_epi16(_mm256_unpacklo_epi8(vs, zero), v_o),
            v128,
            v0101,
        );
        let sc_hi = div255_epi16_avx2(
            _mm256_mullo_epi16(_mm256_unpackhi_epi8(vs, zero), v_o),
            v128,
            v0101,
        );
        let vsc = _mm256_packus_epi16(sc_lo, sc_hi);
        let vd = _mm256_loadu_si256(dst_ptr.add(i) as *const __m256i);
        let v_inv = _mm256_xor_si256(vsc, v_ff);
        let inv_lo = _mm256_shuffle_epi8(v_inv, shuf_lo);
        let inv_hi = _mm256_shuffle_epi8(v_inv, shuf_hi);
        let d_lo = _mm256_unpacklo_epi8(vd, zero);
        let d_hi = _mm256_unpackhi_epi8(vd, zero);
        let s_lo = div255_epi16_avx2(_mm256_mullo_epi16(d_lo, inv_lo), v128, v0101);
        let s_hi = div255_epi16_avx2(_mm256_mullo_epi16(d_hi, inv_hi), v128, v0101);
        let scaled = _mm256_packus_epi16(s_lo, s_hi);
        _mm256_storeu_si256(dst_ptr.add(i) as *mut __m256i, _mm256_adds_epu8(vsc, scaled));
        i += 32;
    }
    blit_row_opacity_scalar(&mut dst_row[i..len], &src_row[i..len], o);
}

/// # Safety
/// Caller must ensure SSE2 is available (guaranteed on x86_64).
#[target_feature(enable = "sse2")]
pub(super) unsafe fn blit_rect_sse2(dst: &mut [u8], src: &[u8], rect: BlitRect) {
    for r in 0..rect.rows {
        let d0 = rect.d_base + r * rect.dst_stride;
        let s0 = rect.s_base + r * rect.src_stride;
        if rect.opacity == 255 {
            blit_row_opaque_sse2(
                &mut dst[d0..d0 + rect.row_bytes],
                &src[s0..s0 + rect.row_bytes],
            );
        } else {
            blit_row_opacity_sse2(
                &mut dst[d0..d0 + rect.row_bytes],
                &src[s0..s0 + rect.row_bytes],
                rect.opacity,
            );
        }
    }
}

/// # Safety
/// Caller must ensure SSE2 is available (guaranteed on x86_64).
#[target_feature(enable = "sse2")]
#[inline]
pub(super) unsafe fn blit_row_opaque_sse2(dst_row: &mut [u8], src_row: &[u8]) {
    let len = dst_row.len().min(src_row.len()) & !3;
    let zero = _mm_setzero_si128();
    let v_ff = _mm_set1_epi8(-1);
    let v255 = _mm_set1_epi16(255);
    let v128 = _mm_set1_epi16(128);
    let v0101 = _mm_set1_epi16(0x0101);
    let mut i = 0;
    let dst_ptr = dst_row.as_mut_ptr();
    let src_ptr = src_row.as_ptr();
    while i + 16 <= len {
        let vs = _mm_loadu_si128(src_ptr.add(i) as *const __m128i);
        if _mm_movemask_epi8(_mm_cmpeq_epi8(vs, zero)) == 0xFFFF {
            i += 16;
            continue;
        }
        if (_mm_movemask_epi8(_mm_cmpeq_epi8(vs, v_ff)) & 0x8888) == 0x8888 {
            _mm_storeu_si128(dst_ptr.add(i) as *mut __m128i, vs);
            i += 16;
            continue;
        }
        let vd = _mm_loadu_si128(dst_ptr.add(i) as *const __m128i);
        let s_lo = _mm_unpacklo_epi8(vs, zero);
        let s_hi = _mm_unpackhi_epi8(vs, zero);
        let inv_lo = _mm_sub_epi16(v255, _mm_shufflehi_epi16::<0xFF>(_mm_shufflelo_epi16::<0xFF>(s_lo)));
        let inv_hi = _mm_sub_epi16(v255, _mm_shufflehi_epi16::<0xFF>(_mm_shufflelo_epi16::<0xFF>(s_hi)));
        let d_lo = _mm_unpacklo_epi8(vd, zero);
        let d_hi = _mm_unpackhi_epi8(vd, zero);
        let sc_lo = div255_epi16_sse2(_mm_mullo_epi16(d_lo, inv_lo), v128, v0101);
        let sc_hi = div255_epi16_sse2(_mm_mullo_epi16(d_hi, inv_hi), v128, v0101);
        let scaled = _mm_packus_epi16(sc_lo, sc_hi);
        _mm_storeu_si128(dst_ptr.add(i) as *mut __m128i, _mm_adds_epu8(vs, scaled));
        i += 16;
    }
    blit_row_opaque_scalar(&mut dst_row[i..len], &src_row[i..len]);
}

/// # Safety
/// Caller must ensure SSE2 is available (guaranteed on x86_64).
#[target_feature(enable = "sse2")]
#[inline]
pub(super) unsafe fn blit_row_opacity_sse2(dst_row: &mut [u8], src_row: &[u8], o: u32) {
    let len = dst_row.len().min(src_row.len()) & !3;
    let zero = _mm_setzero_si128();
    let v_o = _mm_set1_epi16(o as i16);
    let v255 = _mm_set1_epi16(255);
    let v128 = _mm_set1_epi16(128);
    let v0101 = _mm_set1_epi16(0x0101);
    let mut i = 0;
    let dst_ptr = dst_row.as_mut_ptr();
    let src_ptr = src_row.as_ptr();
    while i + 16 <= len {
        let vs = _mm_loadu_si128(src_ptr.add(i) as *const __m128i);
        if _mm_movemask_epi8(_mm_cmpeq_epi8(vs, zero)) == 0xFFFF {
            i += 16;
            continue;
        }
        let sc_lo = div255_epi16_sse2(_mm_mullo_epi16(_mm_unpacklo_epi8(vs, zero), v_o), v128, v0101);
        let sc_hi = div255_epi16_sse2(_mm_mullo_epi16(_mm_unpackhi_epi8(vs, zero), v_o), v128, v0101);
        let vsc = _mm_packus_epi16(sc_lo, sc_hi);
        let inv_lo = _mm_sub_epi16(v255, _mm_shufflehi_epi16::<0xFF>(_mm_shufflelo_epi16::<0xFF>(sc_lo)));
        let inv_hi = _mm_sub_epi16(v255, _mm_shufflehi_epi16::<0xFF>(_mm_shufflelo_epi16::<0xFF>(sc_hi)));
        let vd = _mm_loadu_si128(dst_ptr.add(i) as *const __m128i);
        let d_lo = _mm_unpacklo_epi8(vd, zero);
        let d_hi = _mm_unpackhi_epi8(vd, zero);
        let bd_lo = div255_epi16_sse2(_mm_mullo_epi16(d_lo, inv_lo), v128, v0101);
        let bd_hi = div255_epi16_sse2(_mm_mullo_epi16(d_hi, inv_hi), v128, v0101);
        let scaled = _mm_packus_epi16(bd_lo, bd_hi);
        _mm_storeu_si128(dst_ptr.add(i) as *mut __m128i, _mm_adds_epu8(vsc, scaled));
        i += 16;
    }
    blit_row_opacity_scalar(&mut dst_row[i..len], &src_row[i..len], o);
}

/// # Safety
/// Caller must verify `is_x86_feature_detected!("avx2")`.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn universal_avx2(
    out: &mut [u8],
    a: &[u8],
    b: &[u8],
    rule: &[u8],
    lut: &[u16; 256],
) {
    let pixels = rule
        .len()
        .min(out.len() / 4)
        .min(a.len() / 4)
        .min(b.len() / 4);
    let zero = _mm256_setzero_si256();
    let v256 = _mm256_set1_epi16(256);

    // lut は r に関して単調非増加:
    // r <= max_b_rule なら lut[r] == 256 (純粋に b)、r >= min_a_rule なら lut[r] == 0 (純粋に a)。
    let max_b_rule = lut.iter().rposition(|&v| v == 256);
    let min_a_rule = lut.iter().position(|&v| v == 0);
    let v_max_b = max_b_rule.map(|v| _mm_set1_epi8(v as i8));
    let v_min_a = min_a_rule.map(|v| _mm_set1_epi8(v as i8));

    let mut p = 0;
    let out_ptr = out.as_mut_ptr();
    let a_ptr = a.as_ptr();
    let b_ptr = b.as_ptr();
    let rule_ptr = rule.as_ptr();
    while p + 8 <= pixels {
        let off = p * 4;
        let vr = _mm_loadl_epi64(rule_ptr.add(p) as *const __m128i);
        if let Some(vb_thr) = v_max_b {
            if (_mm_movemask_epi8(_mm_cmpeq_epi8(_mm_max_epu8(vr, vb_thr), vb_thr)) & 0xFF) == 0xFF {
                let vb = _mm256_loadu_si256(b_ptr.add(off) as *const __m256i);
                _mm256_storeu_si256(out_ptr.add(off) as *mut __m256i, vb);
                p += 8;
                continue;
            }
        }
        if let Some(va_thr) = v_min_a {
            if (_mm_movemask_epi8(_mm_cmpeq_epi8(_mm_min_epu8(vr, va_thr), va_thr)) & 0xFF) == 0xFF {
                let va = _mm256_loadu_si256(a_ptr.add(off) as *const __m256i);
                _mm256_storeu_si256(out_ptr.add(off) as *mut __m256i, va);
                p += 8;
                continue;
            }
        }
        let va = _mm256_loadu_si256(a_ptr.add(off) as *const __m256i);
        let vb = _mm256_loadu_si256(b_ptr.add(off) as *const __m256i);
        let rep = |k: usize| (lut[*rule.get_unchecked(p + k) as usize] as i64).wrapping_mul(0x0001_0001_0001_0001);
        let t_lo = _mm256_setr_epi64x(rep(0), rep(1), rep(4), rep(5));
        let t_hi = _mm256_setr_epi64x(rep(2), rep(3), rep(6), rep(7));
        let inv_lo = _mm256_sub_epi16(v256, t_lo);
        let inv_hi = _mm256_sub_epi16(v256, t_hi);
        let a_lo = _mm256_unpacklo_epi8(va, zero);
        let a_hi = _mm256_unpackhi_epi8(va, zero);
        let b_lo = _mm256_unpacklo_epi8(vb, zero);
        let b_hi = _mm256_unpackhi_epi8(vb, zero);
        let lo = _mm256_srli_epi16::<8>(_mm256_add_epi16(
            _mm256_mullo_epi16(a_lo, inv_lo),
            _mm256_mullo_epi16(b_lo, t_lo),
        ));
        let hi = _mm256_srli_epi16::<8>(_mm256_add_epi16(
            _mm256_mullo_epi16(a_hi, inv_hi),
            _mm256_mullo_epi16(b_hi, t_hi),
        ));
        _mm256_storeu_si256(out_ptr.add(off) as *mut __m256i, _mm256_packus_epi16(lo, hi));
        p += 8;
    }
    universal_lut_scalar(&mut out[p * 4..], &a[p * 4..], &b[p * 4..], &rule[p..], lut);
}
