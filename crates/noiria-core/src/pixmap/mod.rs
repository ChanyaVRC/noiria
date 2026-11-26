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
        return Err(format!(
            "画像の寸法が大きすぎます: {w}x{h}（1 辺は {MAX_DIM} 以下）"
        ));
    }
    let bytes = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(4))
        .filter(|&n| n <= MAX_BYTES)
        .ok_or_else(|| {
            format!(
                "画像の寸法が大きすぎます: {w}x{h}（{} MiB 以下）",
                MAX_BYTES >> 20
            )
        })?;
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
                // SAFETY: AVX2 availability was checked above; slices are disjoint and equal length.
                unsafe {
                    simd::premultiply_rgba_avx2_raw(rgba.as_ptr(), data.as_mut_ptr(), rgba.len())
                };
            } else {
                // SAFETY: SSE2 is guaranteed on x86_64; slices are disjoint and equal length.
                unsafe {
                    simd::premultiply_rgba_sse2_raw(rgba.as_ptr(), data.as_mut_ptr(), rgba.len())
                };
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        premultiply_rgba_scalar(rgba, &mut data);
        Self {
            width,
            height,
            data,
        }
    }

    /// 所有するストレートアルファ RGBA8 バッファを in-place でプリマルチプライして包む（追加確保ゼロ）。
    pub fn from_rgba_straight_owned(width: u32, height: u32, mut rgba: Vec<u8>) -> Self {
        assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
        #[cfg(target_arch = "x86_64")]
        {
            let ptr = rgba.as_mut_ptr();
            let len = rgba.len();
            if is_x86_feature_detected!("avx2") {
                // SAFETY: AVX2 availability was checked above; in-place pointer is valid for `len` bytes.
                unsafe { simd::premultiply_rgba_avx2_raw(ptr, ptr, len) };
            } else {
                // SAFETY: SSE2 is guaranteed on x86_64; in-place pointer is valid for `len` bytes.
                unsafe { simd::premultiply_rgba_sse2_raw(ptr, ptr, len) };
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        premultiply_rgba_in_place_scalar(&mut rgba);
        Self {
            width,
            height,
            data: rgba,
        }
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
        let x1 = x.saturating_add(src_w).min(dst_w);
        let y1 = y.saturating_add(src_h).min(dst_h);
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
            let rect = simd::BlitRect {
                d_base,
                dst_stride,
                s_base,
                src_stride,
                row_bytes,
                rows,
                opacity: o,
            };
            if is_x86_feature_detected!("avx2") {
                // SAFETY: AVX2 availability was checked above.
                unsafe {
                    simd::blit_rect_avx2(&mut self.data, &src.data, rect);
                }
            } else {
                // SAFETY: SSE2 is guaranteed on x86_64.
                unsafe {
                    simd::blit_rect_sse2(&mut self.data, &src.data, rect);
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
            let Some(sy) = y.checked_add(row) else {
                break;
            };
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
    for (dst, px) in out
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(rgba.as_chunks::<4>().0)
    {
        let a = px[3] as u32;
        dst[0] = div255(px[0] as u32 * a) as u8;
        dst[1] = div255(px[1] as u32 * a) as u8;
        dst[2] = div255(px[2] as u32 * a) as u8;
        dst[3] = a as u8;
    }
}

#[inline]
fn premultiply_rgba_in_place_scalar(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a == 255 {
            continue;
        }
        px[0] = div255(px[0] as u32 * a) as u8;
        px[1] = div255(px[1] as u32 * a) as u8;
        px[2] = div255(px[2] as u32 * a) as u8;
    }
}

#[inline]
fn blit_row_opaque_scalar(dst_row: &mut [u8], src_row: &[u8]) {
    for (d, s) in dst_row
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(src_row.as_chunks::<4>().0)
    {
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
    for (d, s) in dst_row
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(src_row.as_chunks::<4>().0)
    {
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
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
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
mod simd;

#[cfg(test)]
mod tests;
