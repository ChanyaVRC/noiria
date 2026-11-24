//! グリフカバレッジマスクからの縁取り膨張（van Herk 1D スライディング最大値）と本体 over 合成。

use crate::pixmap::Pixmap;

/// グリフの 8bit カバレッジマスクから、縁取り込みのプリマルチプライ済み絵を作る。
/// （自作ラスタライザ [`crate::font::rasterize`] は常にアルファのみのマスクを返すため、
/// cosmic-text 時代にあった「カラー画像（絵文字等）」の分岐は不要になった。）
/// `bold`: 疑似太字。本体合成ループでのみ左隣ピクセルのカバレッジを拾う
/// （`coverage(x,y) = max(mask(x,y), mask(x-1,y))`）。縁取りループには適用しない
/// （設計判断 5）。advance は呼び出し側で変更しないため、キャンバス拡張も不要。
/// 1 次元スライディング最大値（van Herk / Gil-Werman）。
/// `src`（長さ `w`）の両端を `pad` ゼロ埋めした長さ `ow = w + 2*pad` の列に対し、
/// 半径 `rx`（`rx <= pad`）の区間最大値を `dst`（長さ `ow`）へ書き込む。
fn dilate_row_1d(
    src: &[u8],
    pad: usize,
    rx: usize,
    ext: &mut [u8],
    pre: &mut [u8],
    suf: &mut [u8],
    dst: &mut [u8],
) {
    let ow = dst.len();
    if rx == 0 {
        dst[..pad].fill(0);
        dst[pad..pad + src.len()].copy_from_slice(src);
        dst[pad + src.len()..].fill(0);
        return;
    }
    let ext_len = ow + 2 * rx;
    let ext = &mut ext[..ext_len];
    let pre = &mut pre[..ext_len];
    let suf = &mut suf[..ext_len];
    let left_zero = rx + pad;
    ext[..left_zero].fill(0);
    ext[left_zero..left_zero + src.len()].copy_from_slice(src);
    ext[left_zero + src.len()..].fill(0);

    let k = 2 * rx + 1;
    for (p_chunk, e_chunk) in pre.chunks_mut(k).zip(ext.chunks(k)) {
        let mut m = 0u8;
        for (p, &e) in p_chunk.iter_mut().zip(e_chunk) {
            m = m.max(e);
            *p = m;
        }
    }
    for (s_chunk, e_chunk) in suf.chunks_mut(k).zip(ext.chunks(k)) {
        let mut m = 0u8;
        for (s, &e) in s_chunk.iter_mut().rev().zip(e_chunk.iter().rev()) {
            m = m.max(e);
            *s = m;
        }
    }
    let pre_shift = &pre[2 * rx..2 * rx + ow];
    for ((d, &s), &p) in dst.iter_mut().zip(&suf[..ow]).zip(pre_shift) {
        *d = s.max(p);
    }
}

pub(super) fn rasterize_glyph(
    w: u32,
    h: u32,
    data: &[u8],
    fill: [u8; 4],
    outline_color: [u8; 4],
    outline_width: u32,
    bold: bool,
) -> Pixmap {
    let pad = outline_width as usize;
    let wu = w as usize;
    let hu = h as usize;
    let ow = wu + pad * 2;
    let oh = hu + pad * 2;
    let mut out = Pixmap::new(ow as u32, oh as u32);

    // 1) 縁取り: 円盤 (dx^2 + dy^2 <= pad^2) の最大値フィルタ。
    //    pad <= 2 はスタック行バッファ + 非ゼロ散布でヒープ確保ゼロ。
    //    pad >= 3 は各 dy ごとの水平半径 rx(dy) の van Herk 1D スライディング最大値 + 垂直最大値。
    if outline_width > 0 && outline_color[3] > 0 {
        let mut outline_lut = [[0u8; 4]; 256];
        for (m, slot) in outline_lut.iter_mut().enumerate().skip(1) {
            let a = (m as u32 * outline_color[3] as u32) / 255;
            *slot = [
                (outline_color[0] as u32 * a / 255) as u8,
                (outline_color[1] as u32 * a / 255) as u8,
                (outline_color[2] as u32 * a / 255) as u8,
                a as u8,
            ];
        }

        if pad <= 2 && ow <= 512 {
            let rx_table: [usize; 3] = if pad == 1 { [1, 0, 0] } else { [2, 1, 0] };
            let mut row_max = [0u8; 512];
            let pad_i = pad as i32;
            for oy in 0..oh {
                let row = &mut row_max[..ow];
                row.fill(0);
                let oy_i = oy as i32;
                let sy_min = (oy_i - 2 * pad_i).max(0) as usize;
                let sy_max = oy_i.min(hu as i32 - 1);
                if sy_max >= sy_min as i32 {
                    for sy in sy_min..=(sy_max as usize) {
                        let dy = (sy as i32 - (oy_i - pad_i)).unsigned_abs() as usize;
                        let rx = rx_table[dy];
                        let src_row = &data[sy * wu..(sy + 1) * wu];
                        match rx {
                            0 => {
                                for (dst, &v) in row[pad..pad + wu].iter_mut().zip(src_row) {
                                    *dst = (*dst).max(v);
                                }
                            }
                            1 => {
                                let base = pad - 1;
                                for (sx, &v) in src_row.iter().enumerate() {
                                    if v != 0 {
                                        let o = base + sx;
                                        row[o] = row[o].max(v);
                                        row[o + 1] = row[o + 1].max(v);
                                        row[o + 2] = row[o + 2].max(v);
                                    }
                                }
                            }
                            _ => {
                                // pad == 2, rx == 2 -> base = 0
                                for (sx, &v) in src_row.iter().enumerate() {
                                    if v != 0 {
                                        row[sx] = row[sx].max(v);
                                        row[sx + 1] = row[sx + 1].max(v);
                                        row[sx + 2] = row[sx + 2].max(v);
                                        row[sx + 3] = row[sx + 3].max(v);
                                        row[sx + 4] = row[sx + 4].max(v);
                                    }
                                }
                            }
                        }
                    }
                }
                let out_row = &mut out.data[oy * ow * 4..(oy + 1) * ow * 4];
                for (px, &m) in out_row.as_chunks_mut::<4>().0.iter_mut().zip(row.iter()) {
                    if m != 0 {
                        *px = outline_lut[m as usize];
                    }
                }
            }
        } else {
            let pad_i = outline_width as i32;
            let pad_sq = pad_i * pad_i;
            let mut rx_for_dy = Vec::with_capacity(pad + 1);
            for dy in 0..=pad_i {
                let mut rx = ((pad_sq - dy * dy) as f64).sqrt().floor() as i32;
                while (rx + 1) * (rx + 1) + dy * dy <= pad_sq {
                    rx += 1;
                }
                while rx * rx + dy * dy > pad_sq {
                    rx -= 1;
                }
                rx_for_dy.push(rx as usize);
            }
            let mut unique_rx: Vec<usize> = rx_for_dy.clone();
            unique_rx.sort_unstable();
            unique_rx.dedup();
            let rx_index: Vec<usize> = rx_for_dy
                .iter()
                .map(|r| {
                    unique_rx
                        .binary_search(r)
                        .expect("unique_rx は rx_for_dy から生成されている")
                })
                .collect();

            let num_r = unique_rx.len();
            let mut hmax = vec![0u8; num_r * hu * ow];
            let max_ext = ow + 2 * pad;
            let mut ext = vec![0u8; max_ext];
            let mut pre = vec![0u8; max_ext];
            let mut suf = vec![0u8; max_ext];
            for (ri, &rx) in unique_rx.iter().enumerate() {
                let plane_off = ri * hu * ow;
                for sy in 0..hu {
                    let src_row = &data[sy * wu..(sy + 1) * wu];
                    let dst_row = &mut hmax[plane_off + sy * ow..plane_off + (sy + 1) * ow];
                    dilate_row_1d(src_row, pad, rx, &mut ext, &mut pre, &mut suf, dst_row);
                }
            }

            let mut row_max = vec![0u8; ow];
            for oy in 0..oh {
                row_max.fill(0);
                let oy_i = oy as i32;
                let sy_min = (oy_i - 2 * pad_i).max(0) as usize;
                let sy_max = oy_i.min(hu as i32 - 1);
                if sy_max >= sy_min as i32 {
                    for sy in sy_min..=(sy_max as usize) {
                        let dy = (sy as i32 - (oy_i - pad_i)).unsigned_abs() as usize;
                        let ri = rx_index[dy];
                        let src_row = &hmax[(ri * hu + sy) * ow..(ri * hu + sy + 1) * ow];
                        for (acc, &v) in row_max.iter_mut().zip(src_row) {
                            *acc = (*acc).max(v);
                        }
                    }
                }
                let out_row = &mut out.data[oy * ow * 4..(oy + 1) * ow * 4];
                for (px, &m) in out_row.as_chunks_mut::<4>().0.iter_mut().zip(&row_max) {
                    if m != 0 {
                        *px = outline_lut[m as usize];
                    }
                }
            }
        }
    }

    // 2) 本体を over 合成（bold 時は左隣ピクセルのカバレッジも拾って合成する）
    let mut fill_lut = [(0u32, 0u32, 0u32, 0u32); 256];
    for (m, slot) in fill_lut.iter_mut().enumerate().skip(1) {
        let a = (m as u32 * fill[3] as u32) / 255;
        *slot = (
            fill[0] as u32 * a / 255,
            fill[1] as u32 * a / 255,
            fill[2] as u32 * a / 255,
            a,
        );
    }

    for y in 0..hu {
        let src_row = &data[y * wu..(y + 1) * wu];
        let didx_row = (y + pad) * ow + pad;
        let dst_row = &mut out.data[didx_row * 4..(didx_row + wu) * 4];
        let mut prev_m = 0u8;
        for (dst_px, &cur_m) in dst_row.as_chunks_mut::<4>().0.iter_mut().zip(src_row) {
            let m = if bold {
                let b = cur_m.max(prev_m);
                prev_m = cur_m;
                b
            } else {
                cur_m
            };
            if m == 0 {
                continue;
            }
            let (sr, sg, sb, sa) = fill_lut[m as usize];
            let inv = 255 - sa;
            dst_px[0] = (sr + dst_px[0] as u32 * inv / 255).min(255) as u8;
            dst_px[1] = (sg + dst_px[1] as u32 * inv / 255).min(255) as u8;
            dst_px[2] = (sb + dst_px[2] as u32 * inv / 255).min(255) as u8;
            dst_px[3] = (sa + dst_px[3] as u32 * inv / 255).min(255) as u8;
        }
    }

    out
}
