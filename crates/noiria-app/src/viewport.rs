//! 論理解像度フレームと、任意サイズのクライアント領域との間の写像（レターボックス）。
//! OS 非依存の純関数だけを置く（Win32 / X11 の両ウィンドウ実装と単体テストが共有する）。
//!
//! マウス座標の逆変換を誤ると全クリックがずれるため、ここに隔離して数値テストしている。

#![cfg_attr(windows, allow(dead_code))]

/// クライアント領域内でゲームフレームを表示する矩形（クライアント座標、px）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Viewport {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) w: u32,
    pub(crate) h: u32,
}

/// アスペクト比を保って `client` に収まる最大の矩形を中央寄せで返す（レターボックス）。
pub(crate) fn fit(client_w: u32, client_h: u32, game_w: u32, game_h: u32) -> Viewport {
    let (cw, ch) = (client_w.max(1) as u64, client_h.max(1) as u64);
    let (gw, gh) = (game_w.max(1) as u64, game_h.max(1) as u64);
    let (w, h) = if cw * gh <= ch * gw {
        (cw, ((cw * gh + gw / 2) / gw).clamp(1, ch))
    } else {
        (((ch * gw + gh / 2) / gh).clamp(1, cw), ch)
    };
    Viewport {
        x: ((cw - w) / 2) as i32,
        y: ((ch - h) / 2) as i32,
        w: w as u32,
        h: h as u32,
    }
}

impl Viewport {
    /// クライアント座標 → ゲーム（論理）座標。レターボックスの黒帯上の座標はゲーム範囲の外
    /// （負値や `game_w` 超え）になる。クランプしないのは、黒帯のクリックが端にある UI
    /// （画面下端のメッセージ窓内の選択肢など）の誤爆にならないようにするため。
    pub(crate) fn to_game(self, px: f64, py: f64, game_w: u32, game_h: u32) -> (f64, f64) {
        let w = self.w.max(1) as f64;
        let h = self.h.max(1) as f64;
        (
            (px - self.x as f64) * game_w as f64 / w,
            (py - self.y as f64) * game_h as f64 / h,
        )
    }
}

/// 起動時のクライアントサイズ。`dpi/96` 倍（非整数倍も可）を基本とし、`max_*`
/// （作業領域からウィンドウ枠を引いた、収まる最大のクライアントサイズ）を超える場合は
/// アスペクト比を保って縮める。
pub(crate) fn initial_client_size(
    game_w: u32,
    game_h: u32,
    dpi: u32,
    max_w: u32,
    max_h: u32,
) -> (u32, u32) {
    let dpi = if dpi == 0 { 96 } else { dpi };
    let want = dpi as f64 / 96.0;
    let fit_w = max_w.max(1) as f64 / game_w.max(1) as f64;
    let fit_h = max_h.max(1) as f64 / game_h.max(1) as f64;
    let scale = want.min(fit_w).min(fit_h);
    let w = ((game_w as f64 * scale).round() as u32).max(1);
    let h = ((game_h as f64 * scale).round() as u32).max(1);
    (w, h)
}

/// クライアント領域 (`cw`x`ch`) のうちゲーム領域 `vp` の外側（黒く塗る帯）。左・右・上・下の順で、幅か高さが 0 の帯は含めない。
/// 各辺を独立に判定する（1281x720 のクライアントに 1280x720 を出すと、余りは右の 1px だけで `vp.x` は 0 になる）。
#[cfg(any(windows, test))]
pub(crate) fn letterbox_bands(vp: Viewport, cw: u32, ch: u32) -> Vec<(i32, i32, i32, i32)> {
    let (cw, ch) = (cw as i32, ch as i32);
    let (right, bottom) = (vp.x + vp.w as i32, vp.y + vp.h as i32);
    [
        (0, vp.y, vp.x, vp.h as i32),
        (right, vp.y, cw - right, vp.h as i32),
        (0, 0, cw, vp.y),
        (0, bottom, cw, ch - bottom),
    ]
    .into_iter()
    .filter(|&(_, _, w, h)| w > 0 && h > 0)
    .collect()
}

fn scale_threads() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::thread::available_parallelism()
            .map_or(2, |n| n.get())
            .min(4)
    })
}

/// 表示サイズへ拡大した 1 フレームのキャッシュ。同じ (フレーム世代, 出力サイズ) の再表示（Win32 の WM_PAINT、
/// X11 の Expose / ConfigureNotify）では拡大し直さない。拡大は `noiria_core::scale::scale_bilinear`（Win32/X11 共通）。
#[derive(Default)]
pub(crate) struct ScaleCache {
    buf: Vec<u32>,
    key: Option<(u64, u32, u32)>,
    /// 拡大を実際に行った回数（キャッシュが効いているかのテスト用）。
    pub(crate) scales: u32,
}

impl ScaleCache {
    /// `src`（`sw`x`sh`、世代 `generation`）を `dw`x`dh` へ拡大したものを返す。同サイズなら `src` そのもの。
    pub(crate) fn get<'a>(
        &'a mut self,
        src: &'a [u32],
        generation: u64,
        sw: u32,
        sh: u32,
        dw: u32,
        dh: u32,
    ) -> &'a [u32] {
        if (sw, sh) == (dw, dh)
            || sw == 0
            || sh == 0
            || dw == 0
            || dh == 0
            || src.len() < sw as usize * sh as usize
        {
            return src;
        }
        let key = (generation, dw, dh);
        if self.key != Some(key) {
            self.buf.resize(dw as usize * dh as usize, 0);
            noiria_core::scale::scale_bilinear(
                src,
                sw as usize,
                sh as usize,
                &mut self.buf,
                dw as usize,
                dh as usize,
                scale_threads(),
            );
            self.key = Some(key);
            self.scales += 1;
        }
        &self.buf
    }
}

/// 拡大したフレームを黒帯つきの `dw`x`dh` 全面バッファに置いたもの（X11 のようにサーバ側拡縮・帯塗りが無い経路用）。
/// 出力全体のゼロ埋めはクライアントサイズが変わったときだけ行い、同じ (世代, サイズ) の再表示は何もしない。
#[derive(Default)]
pub(crate) struct LetterboxCache {
    scaled: ScaleCache,
    dst: Vec<u32>,
    dims: (u32, u32),
    key: Option<(u64, u32, u32)>,
    /// 出力全体をゼロ埋めした回数（テスト用）。
    pub(crate) zero_fills: u32,
}

impl LetterboxCache {
    #[cfg(test)]
    pub(crate) fn scales(&self) -> u32 {
        self.scaled.scales
    }

    /// `src`（`sw`x`sh`、世代 `generation`）をレターボックス付きで `dw`x`dh` に置いたバッファと、ゲーム領域を返す。
    pub(crate) fn get(
        &mut self,
        src: &[u32],
        generation: u64,
        sw: u32,
        sh: u32,
        dw: u32,
        dh: u32,
    ) -> (&[u32], Viewport) {
        let vp = fit(dw, dh, sw, sh);
        let key = (generation, dw, dh);
        if self.key == Some(key) {
            return (&self.dst, vp);
        }
        if self.dims != (dw, dh) || self.dst.len() != dw as usize * dh as usize {
            self.dst.clear();
            self.dst.resize(dw as usize * dh as usize, 0);
            self.dims = (dw, dh);
            self.zero_fills += 1;
        }
        self.key = Some(key);
        if src.len() < sw as usize * sh as usize || sw == 0 || sh == 0 {
            return (&self.dst, vp);
        }
        let (vw, dwu) = (vp.w as usize, dw as usize);
        let pixels = self.scaled.get(src, generation, sw, sh, vp.w, vp.h);
        for y in 0..vp.h as usize {
            let d0 = (vp.y as usize + y) * dwu + vp.x as usize;
            self.dst[d0..d0 + vw].copy_from_slice(&pixels[y * vw..y * vw + vw]);
        }
        (&self.dst, vp)
    }
}

/// `LetterboxCache` を使い捨てで使う版（テスト用）。
#[cfg(test)]
pub(crate) fn blit_letterboxed(
    src: &[u32],
    sw: u32,
    sh: u32,
    dst: &mut Vec<u32>,
    dw: u32,
    dh: u32,
) -> Viewport {
    let mut cache = LetterboxCache::default();
    let (px, vp) = cache.get(src, 0, sw, sh, dw, dh);
    dst.clear();
    dst.extend_from_slice(px);
    vp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_identity_when_same_size() {
        assert_eq!(
            fit(1280, 720, 1280, 720),
            Viewport {
                x: 0,
                y: 0,
                w: 1280,
                h: 720
            }
        );
    }

    #[test]
    fn fit_pillarbox_when_client_is_wider() {
        let vp = fit(2000, 720, 1280, 720);
        assert_eq!(
            vp,
            Viewport {
                x: 360,
                y: 0,
                w: 1280,
                h: 720
            }
        );
    }

    #[test]
    fn fit_letterbox_when_client_is_taller() {
        let vp = fit(1280, 1000, 1280, 720);
        assert_eq!(
            vp,
            Viewport {
                x: 0,
                y: 140,
                w: 1280,
                h: 720
            }
        );
    }

    #[test]
    fn fit_fractional_scale_stays_inside_client() {
        let vp = fit(1777, 1000, 1280, 720);
        assert!(vp.x >= 0 && vp.y >= 0);
        assert!(vp.x as u32 + vp.w <= 1777 && vp.y as u32 + vp.h <= 1000);
        assert_eq!(vp.h, 1000);
        assert_eq!(vp.w, 1777);
    }

    #[test]
    fn fit_degenerate_client_never_zero() {
        let vp = fit(0, 0, 1280, 720);
        assert!(vp.w >= 1 && vp.h >= 1);
    }

    #[test]
    fn to_game_identity_and_scaled_corners() {
        let id = fit(1280, 720, 1280, 720);
        assert_eq!(id.to_game(123.0, 45.0, 1280, 720), (123.0, 45.0));

        // 1.5 倍 (1920x1080)
        let vp = fit(1920, 1080, 1280, 720);
        assert_eq!(vp.to_game(0.0, 0.0, 1280, 720), (0.0, 0.0));
        assert_eq!(vp.to_game(1920.0, 1080.0, 1280, 720), (1280.0, 720.0));
        assert_eq!(vp.to_game(960.0, 540.0, 1280, 720), (640.0, 360.0));
        assert_eq!(vp.to_game(1500.0, 900.0, 1280, 720), (1000.0, 600.0));
    }

    #[test]
    fn to_game_accounts_for_letterbox_offset() {
        // 2000x720 のクライアントに 1280x720 → 左右 360px の黒帯。
        let vp = fit(2000, 720, 1280, 720);
        assert_eq!(vp.to_game(360.0, 0.0, 1280, 720), (0.0, 0.0));
        assert_eq!(vp.to_game(1000.0, 360.0, 1280, 720), (640.0, 360.0));
        // 黒帯の上はゲーム範囲外（クランプしない）。
        let (x, _) = vp.to_game(100.0, 360.0, 1280, 720);
        assert!(x < 0.0);
        let (x, _) = vp.to_game(1900.0, 360.0, 1280, 720);
        assert!(x > 1280.0);
    }

    #[test]
    fn to_game_roundtrips_through_forward_mapping() {
        // ゲーム座標 → クライアント座標 → ゲーム座標が一致すること（複数のサイズ・端数倍率）。
        for &(cw, ch) in &[
            (1280u32, 720u32),
            (1920, 1080),
            (1777, 1000),
            (800, 900),
            (3840, 2160),
        ] {
            let vp = fit(cw, ch, 1280, 720);
            for &(gx, gy) in &[(0.0, 0.0), (640.0, 360.0), (1279.0, 719.0), (333.0, 666.0)] {
                let px = vp.x as f64 + gx * vp.w as f64 / 1280.0;
                let py = vp.y as f64 + gy * vp.h as f64 / 720.0;
                let (rx, ry) = vp.to_game(px, py, 1280, 720);
                assert!(
                    (rx - gx).abs() < 1e-9 && (ry - gy).abs() < 1e-9,
                    "{cw}x{ch}"
                );
            }
        }
    }

    #[test]
    fn initial_size_scales_with_dpi_including_fractional() {
        assert_eq!(initial_client_size(1280, 720, 96, 4000, 4000), (1280, 720));
        assert_eq!(
            initial_client_size(1280, 720, 144, 4000, 4000),
            (1920, 1080)
        );
        assert_eq!(initial_client_size(1280, 720, 120, 4000, 4000), (1600, 900));
        assert_eq!(
            initial_client_size(1280, 720, 168, 4000, 4000),
            (2240, 1260)
        );
    }

    #[test]
    fn initial_size_is_capped_to_work_area_keeping_aspect() {
        // 200% 希望だが高さ 1000 までしか使えない → 1000/720 倍。
        let (w, h) = initial_client_size(1280, 720, 192, 1900, 1000);
        assert_eq!(h, 1000);
        assert_eq!(w, 1778);
        // 小さな作業領域では 1 倍未満へ縮む（画面外に出さない）。
        let (w, h) = initial_client_size(1280, 720, 96, 1000, 600);
        assert!(w <= 1000 && h <= 600);
        assert_eq!(initial_client_size(1280, 720, 0, 4000, 4000), (1280, 720));
    }

    #[test]
    fn blit_identity_copies_pixels_exactly() {
        let src: Vec<u32> = (0..12).map(|i| 0x0001_0203 + i).collect();
        let mut dst = Vec::new();
        let vp = blit_letterboxed(&src, 4, 3, &mut dst, 4, 3);
        assert_eq!((vp.w, vp.h), (4, 3));
        assert_eq!(dst, src);
    }

    #[test]
    fn blit_letterbox_bars_are_black_and_content_scales() {
        let src = vec![0x00FF_8040u32; 4 * 2];
        let mut dst = Vec::new();
        // 4x2 → 12x4: 横長クライアントに 8x4 を中央配置（左右 2px の黒帯）。
        let vp = blit_letterboxed(&src, 4, 2, &mut dst, 12, 4);
        assert_eq!(
            vp,
            Viewport {
                x: 2,
                y: 0,
                w: 8,
                h: 4
            }
        );
        for y in 0..4 {
            for x in 0..12 {
                let px = dst[y * 12 + x];
                if (2..10).contains(&x) {
                    assert_eq!(px, 0x00FF_8040, "({x},{y})");
                } else {
                    assert_eq!(px, 0, "bar ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn blit_upscale_interpolates_between_neighbours() {
        // 2x1 の黒→白 を 4x2（同アスペクト）へ。中間は単調増加。
        let src = vec![0x0000_0000u32, 0x00FF_FFFF];
        let mut dst = Vec::new();
        blit_letterboxed(&src, 2, 1, &mut dst, 4, 2);
        let blue: Vec<u32> = dst[..4].iter().map(|p| p & 0xFF).collect();
        assert_eq!(blue[0], 0);
        assert_eq!(blue[3], 255);
        assert!(
            blue[1] < blue[2] && blue[1] > 0 && blue[2] < 255,
            "{blue:?}"
        );
    }

    fn painted(bands: &[(i32, i32, i32, i32)], cw: i32, ch: i32) -> Vec<bool> {
        let mut m = vec![false; (cw * ch) as usize];
        for &(x, y, w, h) in bands {
            for yy in y..y + h {
                for xx in x..x + w {
                    m[(yy * cw + xx) as usize] = true;
                }
            }
        }
        m
    }

    /// 指摘 7: 余りが 1px でも（`vp.x == 0` でも）帯を塗る。旧実装は `vp.x > 0 || vp.y > 0` のときしか塗らず、
    /// 1281x720 / 1280x721 の最後の列/行に古い画素が残った。
    #[test]
    fn letterbox_bands_cover_a_one_pixel_remainder() {
        let vp = fit(1281, 720, 1280, 720);
        assert_eq!((vp.x, vp.w), (0, 1280));
        assert_eq!(letterbox_bands(vp, 1281, 720), vec![(1280, 0, 1, 720)]);

        let vp = fit(1280, 721, 1280, 720);
        assert_eq!((vp.y, vp.h), (0, 720));
        assert_eq!(letterbox_bands(vp, 1280, 721), vec![(0, 720, 1280, 1)]);
    }

    #[test]
    fn letterbox_bands_are_exactly_the_complement_of_the_viewport() {
        for &(cw, ch) in &[
            (1280u32, 720u32),
            (1281, 720),
            (1280, 721),
            (2000, 720),
            (1280, 1000),
            (1777, 1001),
            (3, 2),
        ] {
            let vp = fit(cw, ch, 1280, 720);
            let bands = letterbox_bands(vp, cw, ch);
            let m = painted(&bands, cw as i32, ch as i32);
            for y in 0..ch as i32 {
                for x in 0..cw as i32 {
                    let inside =
                        x >= vp.x && x < vp.x + vp.w as i32 && y >= vp.y && y < vp.y + vp.h as i32;
                    assert_eq!(
                        m[(y * cw as i32 + x) as usize],
                        !inside,
                        "{cw}x{ch} ({x},{y})"
                    );
                }
            }
        }
        assert!(letterbox_bands(fit(1280, 720, 1280, 720), 1280, 720).is_empty());
    }

    fn gradient(w: u32, h: u32) -> Vec<u32> {
        (0..w * h)
            .map(|i| (i.wrapping_mul(2654435761) >> 8) & 0x00FF_FFFF)
            .collect()
    }

    /// 指摘 11: 拡大結果は `scale_bilinear`（Win32 と共通）と画素一致し、帯はゼロ。
    #[test]
    fn letterbox_cache_matches_scale_bilinear_and_leaves_bars_black() {
        let (sw, sh) = (64u32, 36u32);
        let src = gradient(sw, sh);
        for &(dw, dh) in &[(96u32, 54u32), (200, 54), (96, 100), (64, 36), (50, 40)] {
            let mut cache = LetterboxCache::default();
            let (px, vp) = cache.get(&src, 1, sw, sh, dw, dh);
            let mut want = vec![0u32; vp.w as usize * vp.h as usize];
            noiria_core::scale::scale_bilinear(
                &src,
                sw as usize,
                sh as usize,
                &mut want,
                vp.w as usize,
                vp.h as usize,
                1,
            );
            for y in 0..dh as usize {
                for x in 0..dw as usize {
                    let inside = x >= vp.x as usize
                        && x < (vp.x as u32 + vp.w) as usize
                        && y >= vp.y as usize
                        && y < (vp.y as u32 + vp.h) as usize;
                    let got = px[y * dw as usize + x];
                    if inside {
                        assert_eq!(
                            got,
                            want[(y - vp.y as usize) * vp.w as usize + (x - vp.x as usize)],
                            "{dw}x{dh} ({x},{y})"
                        );
                    } else {
                        assert_eq!(got, 0, "bar {dw}x{dh} ({x},{y})");
                    }
                }
            }
        }
    }

    /// 指摘 11: 同じ (フレーム世代, サイズ) の再表示（Expose / ConfigureNotify）では拡大も出力のゼロ埋めもやり直さない。
    /// 新しいフレームでは拡大し直すが、サイズが同じなら出力全体のゼロ埋めはしない。
    #[test]
    fn letterbox_cache_does_not_redo_work_for_the_same_frame_and_size() {
        let (sw, sh) = (64u32, 36u32);
        let a = gradient(sw, sh);
        let b = gradient(sw, sh)
            .iter()
            .map(|p| !p & 0x00FF_FFFF)
            .collect::<Vec<_>>();
        let mut c = LetterboxCache::default();
        let first = c.get(&a, 1, sw, sh, 200, 54).0.to_vec();
        assert_eq!((c.scales(), c.zero_fills), (1, 1));
        for _ in 0..5 {
            assert_eq!(c.get(&a, 1, sw, sh, 200, 54).0, &first[..]);
        }
        assert_eq!((c.scales(), c.zero_fills), (1, 1), "再表示では何もしない");
        let second = c.get(&b, 2, sw, sh, 200, 54).0.to_vec();
        assert_ne!(second, first);
        assert_eq!(
            (c.scales(), c.zero_fills),
            (2, 1),
            "新しいフレームは拡大し直すが、全面ゼロ埋めはしない"
        );
        // 左右の黒帯は前フレームのまま 0（ゼロ埋めし直さなくても汚れない）。
        assert_eq!(second[0], 0);
        // 幅だけ変わる（ビューポートの大きさは同じ）と、拡大は使い回して配置だけやり直す。
        c.get(&b, 2, sw, sh, 220, 54);
        assert_eq!((c.scales(), c.zero_fills), (2, 2));
    }

    #[test]
    fn scale_cache_returns_the_source_when_sizes_match() {
        let src = gradient(8, 4);
        let mut c = ScaleCache::default();
        assert_eq!(c.get(&src, 1, 8, 4, 8, 4).as_ptr(), src.as_ptr());
        assert_eq!(c.scales, 0);
        let big = c.get(&src, 1, 8, 4, 16, 8).to_vec();
        assert_eq!(big.len(), 128);
        c.get(&src, 1, 8, 4, 16, 8);
        assert_eq!(c.scales, 1);
    }
}
