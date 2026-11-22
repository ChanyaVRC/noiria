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
        let mut data = Vec::with_capacity(rgba.len());
        for px in rgba.as_chunks::<4>().0 {
            let a = px[3] as u32;
            data.push(((px[0] as u32 * a + 127) / 255) as u8);
            data.push(((px[1] as u32 * a + 127) / 255) as u8);
            data.push(((px[2] as u32 * a + 127) / 255) as u8);
            data.push(a as u8);
        }
        Self {
            width,
            height,
            data,
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
        let x1 = (x + src_w).min(dst_w);
        let y1 = (y + src_h).min(dst_h);
        if x0 >= x1 || y0 >= y1 {
            return;
        }

        for ty in y0..y1 {
            let sy = (ty - y) as usize;
            let drow = (ty as usize * self.width as usize + x0 as usize) * 4;
            let srow = (sy * src.width as usize + (x0 - x) as usize) * 4;
            let dst_row = &mut self.data[drow..drow + ((x1 - x0) as usize) * 4];
            let src_row = &src.data[srow..srow + ((x1 - x0) as usize) * 4];
            if opacity == 255 {
                // 高速パス: 不透明ピクセルはコピー、透明はスキップ
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
            } else {
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
        }
    }

    /// 2 枚のバッファを t (0.0-1.0) で線形補間して self に書き込む。サイズは同一前提。
    pub fn lerp_from(&mut self, a: &Pixmap, b: &Pixmap, t: f32) {
        let t256 = (t.clamp(0.0, 1.0) * 256.0) as u32;
        let inv = 256 - t256;
        for ((o, pa), pb) in self.data.iter_mut().zip(a.data.iter()).zip(b.data.iter()) {
            *o = ((*pa as u32 * inv + *pb as u32 * t256) >> 8) as u8;
        }
    }

    /// ルール画像によるユニバーサルトランジション合成。
    /// `rule` は self と同サイズの輝度プレーン。式:
    /// a = clamp((t*(255+vague) - rule) / vague, 0, 1)
    /// 内側ループは整数のみ (Q8 固定小数)。
    pub fn universal_from(&mut self, a: &Pixmap, b: &Pixmap, rule: &[u8], t: f32, vague: f32) {
        let vague = vague.max(1.0);
        let phase_q8 = (t.clamp(0.0, 1.0) * (255.0 + vague) * 256.0) as i64;
        let recip_q16 = (65536.0 / vague) as i64; // 1/vague (Q16)
        for (i, ((o, pa), pb)) in self
            .data
            .as_chunks_mut::<4>().0.iter_mut()
            .zip(a.data.as_chunks::<4>().0)
            .zip(b.data.as_chunks::<4>().0)
            .enumerate()
        {
            let diff_q8 = phase_q8 - ((rule[i] as i64) << 8);
            // t256 = clamp(diff/vague, 0, 1) * 256
            let t256 = ((diff_q8 * recip_q16) >> 16).clamp(0, 256) as u32;
            let inv = 256 - t256;
            for c in 0..4 {
                o[c] = ((pa[c] as u32 * inv + pb[c] as u32 * t256) >> 8) as u8;
            }
        }
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
        let sx = self.width as f64 / new_w as f64;
        let sy = self.height as f64 / new_h as f64;
        let src_w = self.width as usize;
        for oy in 0..new_h as usize {
            let y0 = oy as f64 * sy;
            let y1 = y0 + sy;
            for ox in 0..new_w as usize {
                let x0 = ox as f64 * sx;
                let x1 = x0 + sx;
                let mut acc = [0f64; 4];
                let mut weight = 0f64;
                let iy_end = (y1.ceil() as usize).min(self.height as usize);
                let ix_end = (x1.ceil() as usize).min(src_w);
                for iy in (y0.floor() as usize)..iy_end {
                    let wy = (y1.min(iy as f64 + 1.0) - y0.max(iy as f64)).max(0.0);
                    for ix in (x0.floor() as usize)..ix_end {
                        let wx = (x1.min(ix as f64 + 1.0) - x0.max(ix as f64)).max(0.0);
                        let w = wx * wy;
                        let o = (iy * src_w + ix) * 4;
                        for (c, a) in acc.iter_mut().enumerate() {
                            *a += self.data[o + c] as f64 * w;
                        }
                        weight += w;
                    }
                }
                if weight > 0.0 {
                    let o = (oy * new_w as usize + ox) * 4;
                    for (c, a) in acc.iter().enumerate() {
                        out.data[o + c] = (a / weight).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
        out
    }

    /// softbuffer 用の 0x00RRGGBB u32 列へ変換（黒背景へフラット化）。
    pub fn to_bgrx_u32(&self, out: &mut [u32]) {
        for (o, px) in out.iter_mut().zip(self.data.as_chunks::<4>().0) {
            *o = ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | (px[2] as u32);
        }
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
}
