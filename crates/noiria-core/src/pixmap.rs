//! プリマルチプライ済み RGBA8 のピクセルバッファと合成プリミティブ。

/// [trans method=scroll]（W5）の方向。新ページがこの辺から画面内へスライドインする。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollDir {
    Left,
    Right,
    Top,
    Bottom,
}

/// プリマルチプライ済み RGBA8 バッファ。
#[derive(Clone, Debug, PartialEq)]
pub struct Pixmap {
    pub width: u32,
    pub height: u32,
    /// RGBA (premultiplied), row-major, width*height*4 bytes.
    pub data: Vec<u8>,
}

/// スクリプト（や画像ファイル）由来の寸法に課す 1 辺の上限。
pub const MAX_DIM: u32 = 16384;
/// 1 枚のバッファの上限バイト数（`w*h*4`）。16384x16384 は 1GiB になるので辺の上限とは別に面積も縛る。
pub const MAX_BYTES: usize = 256 << 20;

/// 外部入力の寸法 (`w`,`h`) を検査し、確保するバイト数 (`w*h*4`) を返す。辺が `MAX_DIM` 超、
/// または総量が `MAX_BYTES` 超なら Err（0 辺は 0 バイトで許容）。`Pixmap::new` は無検査
/// （`w*h*4` の確保失敗はプロセス abort）なので、外部由来の寸法は必ずこれを通す。
pub fn checked_dims(w: u32, h: u32) -> Result<usize, String> {
    if w > MAX_DIM || h > MAX_DIM {
        return Err(format!("画像の寸法が大きすぎます: {w}x{h}（1 辺は {MAX_DIM} 以下）"));
    }
    let bytes = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(4))
        .filter(|&n| n <= MAX_BYTES)
        .ok_or_else(|| format!("画像の寸法が大きすぎます: {w}x{h}（{} MiB 以下）", MAX_BYTES >> 20))?;
    Ok(bytes)
}

/// round(v / 255)。LLVM が定数除算を乗算+シフトへ最適化するのでこれで十分速い。
#[inline(always)]
fn div255(v: u32) -> u32 {
    (v + 127) / 255
}

impl Pixmap {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; (width as usize) * (height as usize) * 4],
        }
    }

    /// `new` の検査つき版: `checked_dims` を通し、確保失敗も abort せず Err にする。
    pub fn try_new(width: u32, height: u32) -> Result<Self, String> {
        let bytes = checked_dims(width, height)?;
        let mut data = Vec::new();
        data.try_reserve_exact(bytes)
            .map_err(|_| format!("画像のメモリを確保できません: {width}x{height}"))?;
        data.resize(bytes, 0);
        Ok(Self {
            width,
            height,
            data,
        })
    }

    /// ストレートアルファの RGBA8 バッファからプリマルチプライして作成。
    pub fn from_rgba_straight(width: u32, height: u32, rgba: &[u8]) -> Self {
        assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
        let mut data = vec![0u8; rgba.len()];
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                // SAFETY: AVX2 availability was checked above.
                unsafe { simd::premultiply_rgba_avx2(rgba, &mut data) };
            } else {
                // SAFETY: SSE2 is guaranteed on x86_64.
                unsafe { simd::premultiply_rgba_sse2(rgba, &mut data) };
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        premultiply_rgba_scalar(rgba, &mut data);
        Self { width, height, data }
    }

    /// 全画素が不透明 (alpha == 255) な RGBA8 バッファをコピー・乗算なしでそのまま包む。
    pub fn from_rgba_opaque(width: u32, height: u32, data: Vec<u8>) -> Self {
        assert_eq!(data.len(), (width as usize) * (height as usize) * 4);
        debug_assert!(data.as_chunks::<4>().0.iter().all(|px| px[3] == 255));
        Self {
            width,
            height,
            data,
        }
    }

    /// 不透明色 (straight RGBA) で全面塗りつぶし。
    pub fn fill(&mut self, rgba: [u8; 4]) {
        let a = rgba[3] as u32;
        let px = [
            ((rgba[0] as u32 * a + 127) / 255) as u8,
            ((rgba[1] as u32 * a + 127) / 255) as u8,
            ((rgba[2] as u32 * a + 127) / 255) as u8,
            rgba[3],
        ];
        for chunk in self.data.as_chunks_mut::<4>().0 {
            chunk.copy_from_slice(&px);
        }
    }

    /// 角丸矩形を塗る（メッセージボックス背景用）。straight RGBA 指定。
    pub fn fill_rounded_rect(
        &mut self,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        radius: u32,
        rgba: [u8; 4],
    ) {
        let a = rgba[3] as u32;
        let px = [
            ((rgba[0] as u32 * a + 127) / 255) as u8,
            ((rgba[1] as u32 * a + 127) / 255) as u8,
            ((rgba[2] as u32 * a + 127) / 255) as u8,
            rgba[3],
        ];
        let r = radius as i32;
        for dy in 0..h as i32 {
            for dx in 0..w as i32 {
                let tx = x + dx;
                let ty = y + dy;
                if tx < 0 || ty < 0 || tx >= self.width as i32 || ty >= self.height as i32 {
                    continue;
                }
                // 角丸判定
                let cx = if dx < r {
                    r - dx
                } else if dx >= w as i32 - r {
                    dx - (w as i32 - 1 - r)
                } else {
                    0
                };
                let cy = if dy < r {
                    r - dy
                } else if dy >= h as i32 - r {
                    dy - (h as i32 - 1 - r)
                } else {
                    0
                };
                if cx * cx + cy * cy > r * r {
                    continue;
                }
                let idx = ((ty as usize * self.width as usize) + tx as usize) * 4;
                self.data[idx..idx + 4].copy_from_slice(&px);
            }
        }
    }

    /// `src` をこのバッファへ over 合成する。`opacity` は 0-255。
    /// src はプリマルチプライ済みであること。
    pub fn blit_over(&mut self, src: &Pixmap, x: i32, y: i32, opacity: u8) {
        self.blit_over_clipped(src, x, y, opacity, None);
    }

    /// `blit_over` の変種。`max_w` が Some なら src の左端から `max_w` 画素ぶんだけを合成する
    /// （アンダーラインが左から伸びる演出など、レイヤの tween する「見える幅」）。
    pub fn blit_over_clipped(
        &mut self,
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
        let dst_w = self.width as i32;
        let dst_h = self.height as i32;
        let src_w = src.width.min(max_w.unwrap_or(u32::MAX)) as i32;
        let src_h = src.height as i32;

        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + src_w).min(dst_w);
        let y1 = (y + src_h).min(dst_h);
        if x0 >= x1 || y0 >= y1 {
            return;
        }

        let row_bytes = ((x1 - x0) as usize) * 4;
        let dst_stride = self.width as usize * 4;
        let src_stride = src.width as usize * 4;
        let d_base = (y0 as usize * self.width as usize + x0 as usize) * 4;
        let s_base = ((y0 - y) as usize * src.width as usize + (x0 - x) as usize) * 4;
        let rows = (y1 - y0) as usize;

        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                // SAFETY: AVX2 availability was checked above.
                unsafe {
                    simd::blit_rect_avx2(
                        &mut self.data,
                        d_base,
                        dst_stride,
                        &src.data,
                        s_base,
                        src_stride,
                        row_bytes,
                        rows,
                        o,
                    );
                }
            } else {
                // SAFETY: SSE2 is guaranteed on x86_64.
                unsafe {
                    simd::blit_rect_sse2(
                        &mut self.data,
                        d_base,
                        dst_stride,
                        &src.data,
                        s_base,
                        src_stride,
                        row_bytes,
                        rows,
                        o,
                    );
                }
            }
        }

        #[cfg(not(target_arch = "x86_64"))]
        for r in 0..rows {
            let d0 = d_base + r * dst_stride;
            let s0 = s_base + r * src_stride;
            let dst_row = &mut self.data[d0..d0 + row_bytes];
            let src_row = &src.data[s0..s0 + row_bytes];
            if opacity == 255 {
                blit_row_opaque_scalar(dst_row, src_row);
            } else {
                blit_row_opacity_scalar(dst_row, src_row, o);
            }
        }
    }

    /// 2 枚のバッファを t (0.0-1.0) で線形補間して self に書き込む。サイズは同一前提。
    pub fn lerp_from(&mut self, a: &Pixmap, b: &Pixmap, t: f32) {
        let t256 = (t.clamp(0.0, 1.0) * 256.0) as u32;
        let len = self.data.len().min(a.data.len()).min(b.data.len());
        if t256 == 0 {
            self.data[..len].copy_from_slice(&a.data[..len]);
            return;
        }
        if t256 == 256 {
            self.data[..len].copy_from_slice(&b.data[..len]);
            return;
        }
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                // SAFETY: AVX2 availability was checked above.
                unsafe {
                    simd::lerp_bytes_avx2(
                        &mut self.data[..len],
                        &a.data[..len],
                        &b.data[..len],
                        t256,
                    );
                }
            } else {
                // SAFETY: SSE2 is guaranteed on x86_64.
                unsafe {
                    simd::lerp_bytes_sse2(
                        &mut self.data[..len],
                        &a.data[..len],
                        &b.data[..len],
                        t256,
                    );
                }
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        lerp_bytes_scalar(&mut self.data[..len], &a.data[..len], &b.data[..len], t256);
    }

    /// ルール画像によるユニバーサルトランジション合成。
    /// `rule` は self と同サイズの輝度プレーン。式:
    /// a = clamp((t*(255+vague) - rule) / vague, 0, 1)
    /// 内側ループは整数のみ (Q8 固定小数)。
    pub fn universal_from(&mut self, a: &Pixmap, b: &Pixmap, rule: &[u8], t: f32, vague: f32) {
        let vague = vague.max(1.0);
        let phase_q8 = (t.clamp(0.0, 1.0) * (255.0 + vague) * 256.0) as i64;
        let recip_q16 = (65536.0 / vague) as i64; // 1/vague (Q16)
        let mut lut = [0u16; 256];
        for (r, slot) in lut.iter_mut().enumerate() {
            let diff_q8 = phase_q8 - ((r as i64) << 8);
            *slot = ((diff_q8 * recip_q16) >> 16).clamp(0, 256) as u16;
        }
        let pixels = self.data.len().min(a.data.len()).min(b.data.len()) / 4;
        assert!(rule.len() >= pixels);
        let bytes = pixels * 4;
        // lut は r に対して単調非増加: 全画素が a のみ / b のみになる端を短絡する。
        if lut[0] == 0 {
            self.data[..bytes].copy_from_slice(&a.data[..bytes]);
            return;
        }
        if lut[255] == 256 {
            self.data[..bytes].copy_from_slice(&b.data[..bytes]);
            return;
        }
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") {
            // SAFETY: AVX2 availability was checked above.
            unsafe {
                simd::universal_avx2(
                    &mut self.data[..bytes],
                    &a.data[..bytes],
                    &b.data[..bytes],
                    &rule[..pixels],
                    &lut,
                );
            }
            return;
        }
        universal_lut_scalar(
            &mut self.data[..bytes],
            &a.data[..bytes],
            &b.data[..bytes],
            &rule[..pixels],
            &lut,
        );
    }

    /// `(x, y, w, h)` の矩形を切り出して新しい `Pixmap` として返す（`layer_blit_image`
    /// の clip= 用）。範囲外にはみ出す部分は透明（ゼロ埋め）のまま。プリマルチプライ
    /// 済みのデータをそのままコピーするので変換は不要。
    pub fn extract_region(&self, x: u32, y: u32, w: u32, h: u32) -> Pixmap {
        let mut out = Pixmap::new(w, h);
        let copy_w = w.min(self.width.saturating_sub(x)) as usize;
        if copy_w == 0 {
            return out;
        }
        for row in 0..h {
            let sy = y + row;
            if sy >= self.height {
                break;
            }
            let src_off = ((sy as usize * self.width as usize) + x as usize) * 4;
            let dst_off = (row as usize * w as usize) * 4;
            out.data[dst_off..dst_off + copy_w * 4]
                .copy_from_slice(&self.data[src_off..src_off + copy_w * 4]);
        }
        out
    }

    /// [trans method=scroll]（W5）: 新ページ (`new`) が `dir` の辺から `t` (0..1) に応じて
    /// 画面内へスライドインする合成。旧ページ (`old`) は常にその場に留まり、新ページに
    /// 覆われて隠れていく（KAG の `stay=stayfore`/`staybase` 相当の見た目）。
    /// `stay=staynone`（旧ページも押し出されるように動く）は、このエンジンのページ合成が
    /// 常に画面全面を覆う不透明フレーム同士のブレンドである以上、旧ページをずらしても
    /// 可視結果は本質的に変わらない（隙間ができない）ため、値は受理するが描画上は区別しない
    /// （`docs/kag-tag-compat.md`「trans」備考、`docs/implementation-notes.md`「2026-07-11 W5」参照）。
    pub fn scroll_from(&mut self, old: &Pixmap, new: &Pixmap, dir: ScrollDir, t: f32) {
        let t = t.clamp(0.0, 1.0);
        let w = self.width as f32;
        let h = self.height as f32;
        let (nx, ny) = match dir {
            ScrollDir::Left => (-(w * (1.0 - t)).round() as i32, 0),
            ScrollDir::Right => ((w * (1.0 - t)).round() as i32, 0),
            ScrollDir::Top => (0, -(h * (1.0 - t)).round() as i32),
            ScrollDir::Bottom => (0, (h * (1.0 - t)).round() as i32),
        };
        self.fill([0, 0, 0, 255]);
        self.blit_over(old, 0, 0, 255);
        self.blit_over(new, nx, ny, 255);
    }

    /// 左右反転（[image fliplr=]、W5）。プリマルチプライ済みのまま列順序を反転する。
    pub fn flip_h(&self) -> Pixmap {
        let mut out = Pixmap::new(self.width, self.height);
        let w = self.width as usize;
        for y in 0..self.height as usize {
            for x in 0..w {
                let src_off = (y * w + x) * 4;
                let dst_off = (y * w + (w - 1 - x)) * 4;
                out.data[dst_off..dst_off + 4].copy_from_slice(&self.data[src_off..src_off + 4]);
            }
        }
        out
    }

    /// 上下反転（[image flipud=]、W5）。行順序を反転するだけなので行内のプリマルチプライ
    /// 値には触れない。
    pub fn flip_v(&self) -> Pixmap {
        let mut out = Pixmap::new(self.width, self.height);
        let row_bytes = (self.width as usize) * 4;
        for y in 0..self.height as usize {
            let src = &self.data[y * row_bytes..(y + 1) * row_bytes];
            let dst_y = self.height as usize - 1 - y;
            out.data[dst_y * row_bytes..(dst_y + 1) * row_bytes].copy_from_slice(src);
        }
        out
    }

    /// グレースケール化（[image grayscale=]、W5）。輝度 = ITU-R BT.601 近似の固定小数係数
    /// (77, 150, 29)/256 で R/G/B から算出し、RGB を輝度値で置換する（A は不変）。
    /// プリマルチプライ済みのままでも RGB 各チャンネルへ同じ係数を掛けるだけなので、
    /// 乗算済みアルファの性質を壊さない。
    pub fn grayscale(&self) -> Pixmap {
        let mut out = self.clone();
        for px in out.data.as_chunks_mut::<4>().0 {
            let y = ((px[0] as u32 * 77 + px[1] as u32 * 150 + px[2] as u32 * 29) >> 8) as u8;
            px[0] = y;
            px[1] = y;
            px[2] = y;
        }
        out
    }

    /// 面積平均（ボックスフィルタ）で `new_w`x`new_h` へ縮小/拡大する（セーブのサムネイル用）。
    /// 出力画素ごとに対応する入力矩形を小数のまま重み付き平均する（プリマルチプライ済みの
    /// まま平均するのでアルファ縁でも色が滲まない）。幅/高さが 0 の入力・出力は空の Pixmap。
    pub fn resize_area(&self, new_w: u32, new_h: u32) -> Pixmap {
        let mut out = Pixmap::new(new_w, new_h);
        if self.width == 0 || self.height == 0 || new_w == 0 || new_h == 0 {
            return out;
        }
        let src_w = self.width as usize;
        let src_h = self.height as usize;
        let dst_w = new_w as usize;
        let dst_h = new_h as usize;
        let sx = src_w as f64 / dst_w as f64;
        let sy = src_h as f64 / dst_h as f64;

        #[derive(Clone, Copy)]
        struct Span {
            start: usize,
            end: usize,
            w_first: f64,
            w_last: f64,
            w_sum: f64,
        }

        let build_spans = |dst_len: usize, src_len: usize, scale: f64| -> Vec<Span> {
            let mut spans = Vec::with_capacity(dst_len);
            for o in 0..dst_len {
                let p0 = o as f64 * scale;
                let p1 = p0 + scale;
                let start = p0.floor() as usize;
                let end = (p1.ceil() as usize).min(src_len);
                if end <= start {
                    spans.push(Span {
                        start,
                        end,
                        w_first: 0.0,
                        w_last: 0.0,
                        w_sum: 0.0,
                    });
                } else if end == start + 1 {
                    let w = (p1.min(start as f64 + 1.0) - p0.max(start as f64)).max(0.0);
                    spans.push(Span {
                        start,
                        end,
                        w_first: w,
                        w_last: 0.0,
                        w_sum: w,
                    });
                } else {
                    let w_first = (p1.min(start as f64 + 1.0) - p0.max(start as f64)).max(0.0);
                    let last = end - 1;
                    let w_last = (p1.min(last as f64 + 1.0) - p0.max(last as f64)).max(0.0);
                    let middle = (end - start - 2) as f64;
                    spans.push(Span {
                        start,
                        end,
                        w_first,
                        w_last,
                        w_sum: w_first + middle + w_last,
                    });
                }
            }
            spans
        };

        let x_spans = build_spans(dst_w, src_w, sx);
        let y_spans = build_spans(dst_h, src_h, sy);

        let horiz_reduce = |iy: usize, dst_row: &mut [[f64; 4]]| {
            let row = &self.data[iy * src_w * 4..(iy + 1) * src_w * 4];
            let px_row = row.as_chunks::<4>().0;
            for (dst, sp) in dst_row.iter_mut().zip(&x_spans) {
                if sp.end <= sp.start {
                    *dst = [0.0; 4];
                } else if sp.end == sp.start + 1 {
                    let p = px_row[sp.start];
                    let w = sp.w_first;
                    *dst = [
                        p[0] as f64 * w,
                        p[1] as f64 * w,
                        p[2] as f64 * w,
                        p[3] as f64 * w,
                    ];
                } else {
                    let pf = px_row[sp.start];
                    let wf = sp.w_first;
                    let pl = px_row[sp.end - 1];
                    let wl = sp.w_last;
                    let mut m0 = 0u32;
                    let mut m1 = 0u32;
                    let mut m2 = 0u32;
                    let mut m3 = 0u32;
                    for p in &px_row[sp.start + 1..sp.end - 1] {
                        m0 += p[0] as u32;
                        m1 += p[1] as u32;
                        m2 += p[2] as u32;
                        m3 += p[3] as u32;
                    }
                    *dst = [
                        pf[0] as f64 * wf + m0 as f64 + pl[0] as f64 * wl,
                        pf[1] as f64 * wf + m1 as f64 + pl[1] as f64 * wl,
                        pf[2] as f64 * wf + m2 as f64 + pl[2] as f64 * wl,
                        pf[3] as f64 * wf + m3 as f64 + pl[3] as f64 * wl,
                    ];
                }
            }
        };

        let mut cached_iy = usize::MAX;
        let mut cached_h = vec![[0.0f64; 4]; dst_w];
        let mut row_acc = vec![[0.0f64; 4]; dst_w];
        let mut h_tmp = vec![[0.0f64; 4]; dst_w];

        let out_pixels = out.data.as_chunks_mut::<4>().0;
        for (oy, ysp) in y_spans.iter().enumerate() {
            if ysp.end <= ysp.start || ysp.w_sum <= 0.0 {
                continue;
            }
            row_acc.fill([0.0; 4]);
            for iy in ysp.start..ysp.end {
                let wy = if iy == ysp.start {
                    ysp.w_first
                } else if iy == ysp.end - 1 {
                    ysp.w_last
                } else {
                    1.0
                };
                if wy == 0.0 {
                    continue;
                }
                let h_row: &[[f64; 4]] = if iy == cached_iy {
                    &cached_h
                } else if iy == ysp.end - 1 {
                    horiz_reduce(iy, &mut cached_h);
                    cached_iy = iy;
                    &cached_h
                } else {
                    horiz_reduce(iy, &mut h_tmp);
                    &h_tmp
                };
                if wy == 1.0 {
                    for (a, h) in row_acc.iter_mut().zip(h_row) {
                        a[0] += h[0];
                        a[1] += h[1];
                        a[2] += h[2];
                        a[3] += h[3];
                    }
                } else {
                    for (a, h) in row_acc.iter_mut().zip(h_row) {
                        a[0] += h[0] * wy;
                        a[1] += h[1] * wy;
                        a[2] += h[2] * wy;
                        a[3] += h[3] * wy;
                    }
                }
            }
            let out_row = &mut out_pixels[oy * dst_w..(oy + 1) * dst_w];
            for ((dst, acc), xsp) in out_row.iter_mut().zip(&row_acc).zip(&x_spans) {
                let weight = xsp.w_sum * ysp.w_sum;
                if weight > 0.0 {
                    *dst = [
                        (acc[0] / weight).round().clamp(0.0, 255.0) as u8,
                        (acc[1] / weight).round().clamp(0.0, 255.0) as u8,
                        (acc[2] / weight).round().clamp(0.0, 255.0) as u8,
                        (acc[3] / weight).round().clamp(0.0, 255.0) as u8,
                    ];
                }
            }
        }
        out
    }

    /// softbuffer 用の 0x00RRGGBB u32 列へ変換（黒背景へフラット化）。
    pub fn to_bgrx_u32(&self, out: &mut [u32]) {
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                // SAFETY: AVX2 availability was checked above.
                unsafe { simd::to_bgrx_u32_avx2(&self.data, out) };
                return;
            }
            if is_x86_feature_detected!("ssse3") {
                // SAFETY: SSSE3 availability was checked above.
                unsafe { simd::to_bgrx_u32_ssse3(&self.data, out) };
                return;
            }
        }
        to_bgrx_u32_scalar(&self.data, out);
    }
}

#[inline]
fn premultiply_rgba_scalar(rgba: &[u8], out: &mut [u8]) {
    for (dst, px) in out.as_chunks_mut::<4>().0.iter_mut().zip(rgba.as_chunks::<4>().0) {
        let a = px[3] as u32;
        dst[0] = div255(px[0] as u32 * a) as u8;
        dst[1] = div255(px[1] as u32 * a) as u8;
        dst[2] = div255(px[2] as u32 * a) as u8;
        dst[3] = a as u8;
    }
}

#[inline]
fn blit_row_opaque_scalar(dst_row: &mut [u8], src_row: &[u8]) {
    for (d, s) in dst_row.as_chunks_mut::<4>().0.iter_mut().zip(src_row.as_chunks::<4>().0) {
        let sa = s[3] as u32;
        if sa == 255 {
            d.copy_from_slice(s);
        } else if sa != 0 || s[0] | s[1] | s[2] != 0 {
            let inv = 255 - sa;
            for c in 0..4 {
                d[c] = (s[c] as u32 + div255(d[c] as u32 * inv)).min(255) as u8;
            }
        }
    }
}

#[inline]
fn blit_row_opacity_scalar(dst_row: &mut [u8], src_row: &[u8], o: u32) {
    for (d, s) in dst_row.as_chunks_mut::<4>().0.iter_mut().zip(src_row.as_chunks::<4>().0) {
        let sa = div255(s[3] as u32 * o);
        if sa == 0 && s[0] | s[1] | s[2] == 0 {
            continue;
        }
        let inv = 255 - sa;
        for c in 0..4 {
            let sc = div255(s[c] as u32 * o);
            d[c] = (sc + div255(d[c] as u32 * inv)).min(255) as u8;
        }
    }
}

#[inline]
fn lerp_bytes_scalar(out: &mut [u8], a: &[u8], b: &[u8], t256: u32) {
    let inv = 256 - t256;
    for ((o, pa), pb) in out.iter_mut().zip(a.iter()).zip(b.iter()) {
        *o = ((*pa as u32 * inv + *pb as u32 * t256) >> 8) as u8;
    }
}

#[inline]
fn universal_lut_scalar(out: &mut [u8], a: &[u8], b: &[u8], rule: &[u8], lut: &[u16; 256]) {
    for (i, ((o, pa), pb)) in out
        .as_chunks_mut::<4>().0.iter_mut()
        .zip(a.as_chunks::<4>().0)
        .zip(b.as_chunks::<4>().0)
        .enumerate()
    {
        let t256 = lut[rule[i] as usize] as u32;
        let inv = 256 - t256;
        for c in 0..4 {
            o[c] = ((pa[c] as u32 * inv + pb[c] as u32 * t256) >> 8) as u8;
        }
    }
}

#[inline]
fn to_bgrx_u32_scalar(src: &[u8], out: &mut [u32]) {
    for (o, px) in out.iter_mut().zip(src.as_chunks::<4>().0) {
        *o = ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | (px[2] as u32);
    }
}

#[cfg(target_arch = "x86_64")]
mod simd {
    use std::arch::x86_64::*;

    use super::{
        blit_row_opacity_scalar, blit_row_opaque_scalar, lerp_bytes_scalar, premultiply_rgba_scalar,
        to_bgrx_u32_scalar, universal_lut_scalar,
    };

    // For 0 <= v <= 65025, ((v + 128) * 257) >> 16 == ((v + 128) + ((v + 128) >> 8)) >> 8 == (v + 127) / 255.
    // v + 128 <= 65153 fits in u16 without overflow, so vpmulhuw computes exact div255 in 1 instruction.
    #[target_feature(enable = "avx2")]
    #[inline]
    unsafe fn div255_epi16_avx2(v: __m256i, v128: __m256i, v0101: __m256i) -> __m256i {
        _mm256_mulhi_epu16(_mm256_add_epi16(v, v128), v0101)
    }

    #[target_feature(enable = "sse2")]
    #[inline]
    unsafe fn div255_epi16_sse2(v: __m128i, v128: __m128i, v0101: __m128i) -> __m128i {
        _mm_mulhi_epu16(_mm_add_epi16(v, v128), v0101)
    }

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

    #[target_feature(enable = "avx2")]
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn blit_rect_avx2(
        dst: &mut [u8],
        d_base: usize,
        dst_stride: usize,
        src: &[u8],
        s_base: usize,
        src_stride: usize,
        row_bytes: usize,
        rows: usize,
        o: u32,
    ) {
        for r in 0..rows {
            let d0 = d_base + r * dst_stride;
            let s0 = s_base + r * src_stride;
            if o == 255 {
                blit_row_opaque_avx2(&mut dst[d0..d0 + row_bytes], &src[s0..s0 + row_bytes]);
            } else {
                blit_row_opacity_avx2(&mut dst[d0..d0 + row_bytes], &src[s0..s0 + row_bytes], o);
            }
        }
    }

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

    #[target_feature(enable = "sse2")]
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn blit_rect_sse2(
        dst: &mut [u8],
        d_base: usize,
        dst_stride: usize,
        src: &[u8],
        s_base: usize,
        src_stride: usize,
        row_bytes: usize,
        rows: usize,
        o: u32,
    ) {
        for r in 0..rows {
            let d0 = d_base + r * dst_stride;
            let s0 = s_base + r * src_stride;
            if o == 255 {
                blit_row_opaque_sse2(&mut dst[d0..d0 + row_bytes], &src[s0..s0 + row_bytes]);
            } else {
                blit_row_opacity_sse2(&mut dst[d0..d0 + row_bytes], &src[s0..s0 + row_bytes], o);
            }
        }
    }

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

    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn universal_avx2(
        out: &mut [u8],
        a: &[u8],
        b: &[u8],
        rule: &[u8],
        lut: &[u16; 256],
    ) {
        let pixels = rule.len();
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_dims_rejects_oversized_and_wrapping_dimensions() {
        assert_eq!(checked_dims(1280, 720), Ok(1280 * 720 * 4));
        assert_eq!(checked_dims(0, 5), Ok(0));
        assert_eq!(checked_dims(MAX_DIM, 4096), Ok(MAX_DIM as usize * 4096 * 4));
        for (w, h) in [(100_000, 100_000), (1 << 31, 1 << 31), (u32::MAX, 1), (MAX_DIM + 1, 1), (MAX_DIM, MAX_DIM)] {
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
        assert!(small.data.as_chunks::<4>().0.iter().all(|p| *p == [10, 20, 30, 255]));

        // 左半分が黒・右半分が白の 4x1 を 2x1 へ: 各画素は元の 2 画素の平均。
        let mut p = Pixmap::new(4, 1);
        p.data = vec![0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255];
        assert_eq!(p.resize_area(2, 1).data, vec![0, 0, 0, 255, 255, 255, 255, 255]);
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
                *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
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
            let pa = Pixmap { width: len as u32, height: 1, data: a_bytes.clone() };
            let pb = Pixmap { width: len as u32, height: 1, data: b_bytes.clone() };
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

            #[cfg(target_arch = "x86_64")]
            {
                if is_x86_feature_detected!("avx2") {
                    let mut got_avx2 = vec![0u8; rgba.len()];
                    // SAFETY: AVX2 was checked above.
                    unsafe { simd::premultiply_rgba_avx2(&rgba, &mut got_avx2) };
                    assert_eq!(got_avx2, want, "avx2 premul pixels={pixels}");
                }
                let mut got_sse2 = vec![0u8; rgba.len()];
                // SAFETY: SSE2 is guaranteed on x86_64.
                unsafe { simd::premultiply_rgba_sse2(&rgba, &mut got_sse2) };
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
                blit_row_opaque_scalar(&mut dst.data[drow..drow + row_bytes], &src.data[srow..srow + row_bytes]);
            } else {
                blit_row_opacity_scalar(&mut dst.data[drow..drow + row_bytes], &src.data[srow..srow + row_bytes], o);
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
                for (k, px) in src_row[96..128].as_chunks_mut::<4>().0.iter_mut().enumerate() {
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
                assert_eq!(got.data, want.data, "clipped x={x} y={y} op={op} max_w={max_w:?}");
            }
        }
    }

    #[test]
    fn universal_from_simd_matches_original_scalar() {
        let mut seed = 777u64;
        for pixels in [0usize, 1, 7, 8, 9, 17, 33, 257] {
            let a = Pixmap { width: pixels as u32, height: 1, data: lcg_bytes(&mut seed, pixels * 4) };
            let b = Pixmap { width: pixels as u32, height: 1, data: lcg_bytes(&mut seed, pixels * 4) };
            let rule: Vec<u8> = (0..pixels).map(|i| (i % 256) as u8).collect();
            for &(t, vague) in &[(0.0f32, 1.0f32), (0.25, 16.0), (0.5, 64.0), (0.75, 255.0), (1.0, 0.5)] {
                let mut want = vec![0u8; pixels * 4];
                let v = vague.max(1.0);
                let phase_q8 = (t.clamp(0.0, 1.0) * (255.0 + v) * 256.0) as i64;
                let recip_q16 = (65536.0 / v) as i64;
                for i in 0..pixels {
                    let diff_q8 = phase_q8 - ((rule[i] as i64) << 8);
                    let t256 = ((diff_q8 * recip_q16) >> 16).clamp(0, 256) as u32;
                    let inv = 256 - t256;
                    for c in 0..4 {
                        want[i * 4 + c] = ((a.data[i * 4 + c] as u32 * inv + b.data[i * 4 + c] as u32 * t256) >> 8) as u8;
                    }
                }
                let mut got = Pixmap::new(pixels as u32, 1);
                got.universal_from(&a, &b, &rule, t, vague);
                assert_eq!(got.data, want, "universal pixels={pixels} t={t} vague={vague}");
            }
        }
    }
}
