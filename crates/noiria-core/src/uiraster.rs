//! UI 用パネルのソフトウェアラスタライザ（縦/横グラデーション・角丸・枠線・ソフトシャドウ）。
//!
//! 角丸矩形の符号付き距離（SDF）から画素ごとのカバレッジを求めるので、縁はアンチエイリアスされ、
//! 影は距離に応じて滑らかに減衰する。出力はプリマルチプライ済み（`Pixmap` の規約）。
//! すべて f32 の純関数で、同じ入力は常に同じ画素になる（V8/tsint の両エンジンから同じ ops で
//! 呼ばれるのでフレームが一致する）。

use crate::pixmap::Pixmap;

/// `render_panel` の指定。色は straight RGBA。`pad` はパネル矩形の外側に確保する余白
/// （影が収まる大きさ。出力は (w + 2*pad) x (h + 2*pad)、パネル本体は (pad, pad) を左上とする）。
#[derive(Clone, Debug, PartialEq)]
pub struct PanelSpec {
    pub w: u32,
    pub h: u32,
    pub radius: f32,
    pub top: [u8; 4],
    pub bottom: [u8; 4],
    /// true なら左→右のグラデーション（top が左端、bottom が右端）。
    pub horizontal: bool,
    pub border: [u8; 4],
    pub border_w: f32,
    pub shadow_blur: f32,
    pub shadow_dy: f32,
    /// 影の濃さ (0..=1)。
    pub shadow_alpha: f32,
    pub pad: u32,
    /// 0 より大きければ、グラデーションは先頭の `ramp` px だけで top→bottom へ（滑らかに）変わり、
    /// 以降は bottom の単色。0 は全長で線形（暗幕の「上端が溶ける」帯に使う）。
    pub ramp: f32,
}

impl PanelSpec {
    /// ops から届く数値列（`api.layer.panel` が組み立てる）を解釈する。足りなければ None。
    pub fn from_slice(v: &[f64]) -> Option<PanelSpec> {
        if v.len() < 21 {
            return None;
        }
        let u = |i: usize| v[i].clamp(0.0, 4096.0) as u32;
        let c = |i: usize| -> [u8; 4] {
            [
                v[i].clamp(0.0, 255.0) as u8,
                v[i + 1].clamp(0.0, 255.0) as u8,
                v[i + 2].clamp(0.0, 255.0) as u8,
                v[i + 3].clamp(0.0, 255.0) as u8,
            ]
        };
        Some(PanelSpec {
            w: u(0),
            h: u(1),
            radius: v[2].max(0.0) as f32,
            top: c(3),
            bottom: c(7),
            border: c(11),
            border_w: v[15].max(0.0) as f32,
            shadow_blur: v[16].max(0.0) as f32,
            shadow_dy: v[17] as f32,
            shadow_alpha: v[18].clamp(0.0, 1.0) as f32,
            pad: u(19),
            horizontal: v[20] != 0.0,
            ramp: v.get(21).copied().unwrap_or(0.0).max(0.0) as f32,
        })
    }
}

/// 角丸矩形（中心 c、半サイズ half、角半径 r）までの符号付き距離。内側が負。
fn sdf_round_rect(px: f32, py: f32, cx: f32, cy: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hx - r);
    let qy = (py - cy).abs() - (hy - r);
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn grad_at(s: &PanelSpec, along: f32) -> (f32, f32, f32, f32) {
    let t = if s.ramp > 0.0 {
        smoothstep(0.0, 1.0, along / s.ramp)
    } else {
        (along / if s.horizontal { s.w } else { s.h } as f32).clamp(0.0, 1.0)
    };
    (
        lerp(s.top[0] as f32, s.bottom[0] as f32, t),
        lerp(s.top[1] as f32, s.bottom[1] as f32, t),
        lerp(s.top[2] as f32, s.bottom[2] as f32, t),
        lerp(s.top[3] as f32, s.bottom[3] as f32, t) / 255.0,
    )
}

fn quantize_fill((fr, fg, fb, fa): (f32, f32, f32, f32)) -> [u8; 4] {
    if fa > 0.0 {
        [
            (fr * fa).round().clamp(0.0, 255.0) as u8,
            (fg * fa).round().clamp(0.0, 255.0) as u8,
            (fb * fa).round().clamp(0.0, 255.0) as u8,
            (fa * 255.0).round().clamp(0.0, 255.0) as u8,
        ]
    } else {
        [0, 0, 0, 0]
    }
}

/// 角丸も枠も影も無いパネル（暗幕・線・バー）の高速経路。
fn fill_flat(s: &PanelSpec, out: &mut Pixmap) {
    let ow = out.width as usize;
    let pad = s.pad as usize;
    let w = s.w as usize;
    let pixels = out.data.as_chunks_mut::<4>().0;
    if s.horizontal {
        let cols: Vec<[u8; 4]> = (0..s.w)
            .map(|x| quantize_fill(grad_at(s, x as f32 + 0.5)))
            .collect();
        for y in 0..s.h as usize {
            let row = (pad + y) * ow + pad;
            pixels[row..row + w].copy_from_slice(&cols);
        }
    } else {
        for y in 0..s.h as usize {
            let c = quantize_fill(grad_at(s, y as f32 + 0.5));
            let row = (pad + y) * ow + pad;
            pixels[row..row + w].fill(c);
        }
    }
}

/// パネルを描いた Pixmap を返す。w/h が 0 なら空（pad のみ）の Pixmap。
pub fn render_panel(s: &PanelSpec) -> Pixmap {
    let ow = s.w + s.pad * 2;
    let oh = s.h + s.pad * 2;
    let mut out = Pixmap::new(ow, oh);
    if s.w == 0 || s.h == 0 {
        return out;
    }
    let hx = s.w as f32 / 2.0;
    let hy = s.h as f32 / 2.0;
    let r = s.radius.min(hx).min(hy);
    let cx = s.pad as f32 + hx;
    let cy = s.pad as f32 + hy;
    let has_border = s.border_w > 0.0 && s.border[3] > 0;
    let has_shadow = s.shadow_blur > 0.0 && s.shadow_alpha > 0.0 && s.pad > 0;
    if r <= 0.0 && !has_border && !has_shadow {
        fill_flat(s, &mut out);
        return out;
    }

    // 内側フラット領域（cov==1, inner==1, ring==0, sa==0 が確実に成立する矩形）:
    // SDF/影計算を一切行わず、グラデーション塗りだけを一括で行う。
    let inset = r.max(if has_border { s.border_w } else { 0.0 }) + 1.0;
    let pad_f = s.pad as f32;
    let inner_x0 = ((pad_f + inset).ceil().max(0.0) as u32).min(ow);
    let inner_x1 = ((pad_f + s.w as f32 - inset).floor().max(0.0) as u32).min(ow);
    let inner_y0 = ((pad_f + inset).ceil().max(0.0) as u32).min(oh);
    let inner_y1 = ((pad_f + s.h as f32 - inset).floor().max(0.0) as u32).min(oh);
    let has_inner = inner_x0 < inner_x1 && inner_y0 < inner_y1;

    // 縦グラデーション時、上下の直線枠・影帯では |px - cx| <= hx - r の範囲で
    // qx <= 0 となり SDF・影距離ともに x 非依存になるため、1 画素だけ計算して fill できる。
    let flat_x0 = ((cx - (hx - r)).ceil().max(0.0) as u32).min(ow);
    let flat_x1 = ((cx + (hx - r)).floor().max(0.0) as u32).min(ow);
    let has_flat_band = !s.horizontal && flat_x0 < flat_x1;

    // 影も本体も届かない外側領域のカリング境界。
    let reach_x =
        hx + if has_shadow {
            s.shadow_blur.max(0.5)
        } else {
            0.5
        } + 0.5;
    let min_x = ((cx - reach_x).floor().max(0.0) as u32).min(ow);
    let max_x = ((cx + reach_x).ceil().max(0.0) as u32).min(ow);

    let horiz_grad: Vec<(f32, f32, f32, f32)> = if s.horizontal {
        (0..ow)
            .map(|ox| grad_at(s, ox as f32 + 0.5 - pad_f))
            .collect()
    } else {
        Vec::new()
    };
    let horiz_px: Vec<[u8; 4]> = if s.horizontal && has_inner {
        horiz_grad.iter().map(|&g| quantize_fill(g)).collect()
    } else {
        Vec::new()
    };

    let border_r = s.border[0] as f32;
    let border_g = s.border[1] as f32;
    let border_b = s.border[2] as f32;
    let border_a = s.border[3] as f32 / 255.0;
    let shadow_e0 = -s.shadow_blur * 0.25;

    let shade_pixel = |ox: u32, py: f32, row_grad: (f32, f32, f32, f32), dst: &mut [u8; 4]| {
        let px = ox as f32 + 0.5;
        let d = sdf_round_rect(px, py, cx, cy, hx, hy, r);
        let cov = (0.5 - d).clamp(0.0, 1.0);
        let (mut pr, mut pg, mut pb, mut pa) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        if cov > 0.0 {
            let (fr, fg, fb, fa) = if s.horizontal {
                horiz_grad[ox as usize]
            } else {
                row_grad
            };
            let inner = if has_border {
                (0.5 - (d + s.border_w)).clamp(0.0, 1.0).min(cov)
            } else {
                cov
            };
            let ring = cov - inner;
            let fill_a = fa * inner;
            pr += fr * fill_a;
            pg += fg * fill_a;
            pb += fb * fill_a;
            pa += fill_a;
            if ring > 0.0 {
                let ba = border_a * ring;
                let inv = 1.0 - ba;
                pr = pr * inv + border_r * ba;
                pg = pg * inv + border_g * ba;
                pb = pb * inv + border_b * ba;
                pa = pa * inv + ba;
            }
        }
        if has_shadow && cov < 1.0 {
            let sd = sdf_round_rect(px, py - s.shadow_dy, cx, cy, hx, hy, r);
            if sd < s.shadow_blur {
                let sa = s.shadow_alpha * (1.0 - smoothstep(shadow_e0, s.shadow_blur, sd));
                let sa = sa * (1.0 - cov);
                if sa > 0.0 {
                    let inv = 1.0 - pa;
                    pa += sa * inv;
                }
            }
        }
        if pa > 0.0 {
            *dst = [
                pr.round().clamp(0.0, 255.0) as u8,
                pg.round().clamp(0.0, 255.0) as u8,
                pb.round().clamp(0.0, 255.0) as u8,
                (pa * 255.0).round().clamp(0.0, 255.0) as u8,
            ];
        }
    };

    for oy in 0..oh {
        let py = oy as f32 + 0.5;
        let dy_body = (py - cy).abs();
        let dy_shadow = (py - s.shadow_dy - cy).abs();
        if dy_body > hy + 0.5 && (!has_shadow || dy_shadow > hy + s.shadow_blur + 0.5) {
            continue;
        }
        let row_grad = if s.horizontal {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            grad_at(s, py - pad_f)
        };
        let row_pixels =
            &mut out.data.as_chunks_mut::<4>().0[(oy * ow) as usize..((oy + 1) * ow) as usize];

        if has_inner && oy >= inner_y0 && oy < inner_y1 {
            for ox in min_x..inner_x0 {
                shade_pixel(ox, py, row_grad, &mut row_pixels[ox as usize]);
            }
            if s.horizontal {
                row_pixels[inner_x0 as usize..inner_x1 as usize]
                    .copy_from_slice(&horiz_px[inner_x0 as usize..inner_x1 as usize]);
            } else {
                let c = quantize_fill(row_grad);
                if c[3] > 0 {
                    row_pixels[inner_x0 as usize..inner_x1 as usize].fill(c);
                }
            }
            for ox in inner_x1..max_x {
                shade_pixel(ox, py, row_grad, &mut row_pixels[ox as usize]);
            }
        } else if has_flat_band && dy_body >= hy - r && (!has_shadow || dy_shadow >= hy - r) {
            let fx0 = flat_x0.max(min_x);
            let fx1 = flat_x1.min(max_x);
            for ox in min_x..fx0 {
                shade_pixel(ox, py, row_grad, &mut row_pixels[ox as usize]);
            }
            if fx0 < fx1 {
                let mut sample = [0u8; 4];
                shade_pixel(fx0, py, row_grad, &mut sample);
                if sample[3] > 0 {
                    row_pixels[fx0 as usize..fx1 as usize].fill(sample);
                }
            }
            for ox in fx1..max_x {
                shade_pixel(ox, py, row_grad, &mut row_pixels[ox as usize]);
            }
        } else {
            for ox in min_x..max_x {
                shade_pixel(ox, py, row_grad, &mut row_pixels[ox as usize]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(w: u32, h: u32) -> PanelSpec {
        PanelSpec {
            w,
            h,
            radius: 0.0,
            top: [200, 100, 50, 255],
            bottom: [200, 100, 50, 255],
            horizontal: false,
            border: [0, 0, 0, 0],
            border_w: 0.0,
            shadow_blur: 0.0,
            shadow_dy: 0.0,
            shadow_alpha: 0.0,
            pad: 0,
            ramp: 0.0,
        }
    }

    fn px(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * p.width + x) * 4) as usize;
        [p.data[i], p.data[i + 1], p.data[i + 2], p.data[i + 3]]
    }

    #[test]
    fn solid_rect_fills_exactly() {
        let p = render_panel(&spec(8, 6));
        assert_eq!((p.width, p.height), (8, 6));
        for y in 0..6 {
            for x in 0..8 {
                assert_eq!(px(&p, x, y), [200, 100, 50, 255]);
            }
        }
    }

    #[test]
    fn vertical_gradient_interpolates_alpha_and_is_premultiplied() {
        let mut s = spec(4, 100);
        s.top = [255, 255, 255, 0];
        s.bottom = [255, 255, 255, 255];
        let p = render_panel(&s);
        assert!(px(&p, 1, 0)[3] < 6, "上端はほぼ透明");
        assert!(px(&p, 1, 99)[3] > 249, "下端はほぼ不透明");
        let mid = px(&p, 1, 50);
        assert!((120..=135).contains(&mid[3]));
        // 白のプリマルチプライ済み: rgb == a
        assert_eq!(mid[0], mid[3]);
        let a = px(&p, 1, 20)[3];
        let b = px(&p, 1, 80)[3];
        assert!(b > a, "下ほど濃い");
    }

    #[test]
    fn ramp_limits_the_gradient_to_the_first_pixels() {
        let mut s = spec(4, 100);
        s.top = [255, 255, 255, 0];
        s.bottom = [255, 255, 255, 200];
        s.ramp = 40.0;
        let p = render_panel(&s);
        assert!(px(&p, 1, 0)[3] < 3);
        assert_eq!(px(&p, 1, 40)[3], 200, "ramp の終わりで bottom に達する");
        assert_eq!(px(&p, 1, 99)[3], 200, "以降は単色");
        let q = px(&p, 1, 10)[3];
        assert!(q < 40, "イーズイン: 序盤は緩やか {q}");
    }

    #[test]
    fn horizontal_gradient_runs_left_to_right() {
        let mut s = spec(100, 4);
        s.top = [0, 0, 0, 255];
        s.bottom = [255, 255, 255, 255];
        s.horizontal = true;
        let p = render_panel(&s);
        assert!(px(&p, 2, 1)[0] < 10);
        assert!(px(&p, 97, 1)[0] > 245);
    }

    #[test]
    fn rounded_corners_are_transparent_and_antialiased() {
        let mut s = spec(40, 40);
        s.radius = 16.0;
        let p = render_panel(&s);
        assert_eq!(px(&p, 0, 0)[3], 0, "角は抜ける");
        assert_eq!(px(&p, 20, 20), [200, 100, 50, 255]);
        // 角の円弧付近には 0 < a < 255 の画素がある
        let mut partial = 0;
        for y in 0..16 {
            for x in 0..16 {
                let a = px(&p, x, y)[3];
                if a > 0 && a < 255 {
                    partial += 1;
                }
            }
        }
        assert!(partial >= 6, "円弧が滑らかでない: {partial}");
    }

    #[test]
    fn border_ring_has_border_color_and_interior_keeps_fill() {
        let mut s = spec(30, 30);
        s.border = [255, 255, 255, 255];
        s.border_w = 2.0;
        s.radius = 6.0;
        let p = render_panel(&s);
        assert_eq!(px(&p, 15, 0), [255, 255, 255, 255], "上辺は枠線");
        assert_eq!(px(&p, 0, 15), [255, 255, 255, 255], "左辺は枠線");
        assert_eq!(px(&p, 15, 15), [200, 100, 50, 255], "中は塗り");
    }

    #[test]
    fn shadow_lives_in_the_pad_only_and_fades_out() {
        let mut s = spec(20, 20);
        s.pad = 16;
        s.shadow_blur = 12.0;
        s.shadow_alpha = 0.6;
        s.shadow_dy = 4.0;
        let p = render_panel(&s);
        assert_eq!((p.width, p.height), (52, 52));
        // 本体の内側は塗りのまま（影が透けて色が変わらない）
        assert_eq!(px(&p, 26, 26), [200, 100, 50, 255]);
        // 下側の影は上側より濃く、遠いほど薄い
        let below_near = px(&p, 26, 16 + 20 + 1)[3];
        let below_far = px(&p, 26, 16 + 20 + 10)[3];
        let above_near = px(&p, 26, 16 - 2)[3];
        assert!(below_near > below_far, "{below_near} {below_far}");
        assert!(below_near > above_near, "dy で下へずれる");
        assert!(below_near > 20);
        assert_eq!(px(&p, 0, 0)[3], 0, "最外周は影が届かない");
        // 影は黒
        let i = ((16 + 20 + 1) * 52 + 26) * 4;
        assert_eq!(&p.data[i..i + 3], &[0, 0, 0]);
    }

    #[test]
    fn flat_fast_path_matches_the_general_path_pixel_for_pixel() {
        let mut variants = Vec::new();
        for (horizontal, ramp, pad) in [
            (false, 0.0, 0),
            (false, 28.0, 3),
            (true, 0.0, 2),
            (false, 0.0, 9),
        ] {
            let mut s = spec(37, 53);
            s.top = [6, 9, 24, 242];
            s.bottom = [250, 190, 30, 17];
            s.horizontal = horizontal;
            s.ramp = ramp;
            s.pad = pad;
            variants.push(s);
        }
        for s in variants {
            let fast = render_panel(&s);
            // 角丸の半径をほぼ 0 にして一般経路を強制する（半径が 0 のときだけ高速経路に入る）
            let mut g = PanelSpec {
                radius: 1e-7,
                ..s.clone()
            };
            g.border_w = 0.0;
            let slow = render_panel(&g);
            assert_eq!((fast.width, fast.height), (slow.width, slow.height));
            assert_eq!(
                fast.data, slow.data,
                "horizontal={} ramp={} pad={}",
                s.horizontal, s.ramp, s.pad
            );
        }
    }

    #[test]
    fn from_slice_rejects_short_input_and_parses_fields() {
        assert!(PanelSpec::from_slice(&[1.0; 5]).is_none());
        let v = [
            10.0, 20.0, 4.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 1.5,
            8.0, 2.0, 0.5, 6.0, 1.0, 30.0,
        ];
        let s = PanelSpec::from_slice(&v).unwrap();
        assert_eq!((s.w, s.h, s.pad), (10, 20, 6));
        assert_eq!(s.top, [1, 2, 3, 4]);
        assert_eq!(s.bottom, [5, 6, 7, 8]);
        assert_eq!(s.border, [9, 10, 11, 12]);
        assert!(s.horizontal);
        assert_eq!(s.ramp, 30.0);
        assert!((s.shadow_alpha - 0.5).abs() < 1e-6);
    }

    /// 最適化前の全画素 SDF 参照実装と画素単位で完全一致すること。
    #[test]
    fn optimized_render_panel_matches_naive_sdf_pixel_for_pixel() {
        fn render_naive(s: &PanelSpec) -> Pixmap {
            let ow = s.w + s.pad * 2;
            let oh = s.h + s.pad * 2;
            let mut out = Pixmap::new(ow, oh);
            if s.w == 0 || s.h == 0 {
                return out;
            }
            let hx = s.w as f32 / 2.0;
            let hy = s.h as f32 / 2.0;
            let r = s.radius.min(hx).min(hy);
            let cx = s.pad as f32 + hx;
            let cy = s.pad as f32 + hy;
            let has_border = s.border_w > 0.0 && s.border[3] > 0;
            let has_shadow = s.shadow_blur > 0.0 && s.shadow_alpha > 0.0 && s.pad > 0;
            for oy in 0..oh {
                let py = oy as f32 + 0.5;
                for ox in 0..ow {
                    let px = ox as f32 + 0.5;
                    let d = sdf_round_rect(px, py, cx, cy, hx, hy, r);
                    let cov = (0.5 - d).clamp(0.0, 1.0);
                    let (mut pr, mut pg, mut pb, mut pa) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
                    if cov > 0.0 {
                        let along = if s.horizontal {
                            px - s.pad as f32
                        } else {
                            py - s.pad as f32
                        };
                        let t = if s.ramp > 0.0 {
                            smoothstep(0.0, 1.0, along / s.ramp)
                        } else {
                            (along / if s.horizontal { s.w } else { s.h } as f32).clamp(0.0, 1.0)
                        };
                        let fr = lerp(s.top[0] as f32, s.bottom[0] as f32, t);
                        let fg = lerp(s.top[1] as f32, s.bottom[1] as f32, t);
                        let fb = lerp(s.top[2] as f32, s.bottom[2] as f32, t);
                        let fa = lerp(s.top[3] as f32, s.bottom[3] as f32, t) / 255.0;
                        let inner = if has_border {
                            (0.5 - (d + s.border_w)).clamp(0.0, 1.0).min(cov)
                        } else {
                            cov
                        };
                        let ring = cov - inner;
                        let fill_a = fa * inner;
                        pr += fr * fill_a;
                        pg += fg * fill_a;
                        pb += fb * fill_a;
                        pa += fill_a;
                        if ring > 0.0 {
                            let ba = s.border[3] as f32 / 255.0 * ring;
                            let inv = 1.0 - ba;
                            pr = pr * inv + s.border[0] as f32 * ba;
                            pg = pg * inv + s.border[1] as f32 * ba;
                            pb = pb * inv + s.border[2] as f32 * ba;
                            pa = pa * inv + ba;
                        }
                    }
                    if has_shadow {
                        let sd = sdf_round_rect(px, py - s.shadow_dy, cx, cy, hx, hy, r);
                        let sa = s.shadow_alpha
                            * (1.0 - smoothstep(-s.shadow_blur * 0.25, s.shadow_blur, sd));
                        let sa = sa * (1.0 - cov);
                        if sa > 0.0 {
                            let inv = 1.0 - pa;
                            pa += sa * inv;
                        }
                    }
                    if pa > 0.0 {
                        let i = ((oy * ow + ox) * 4) as usize;
                        out.data[i] = pr.round().clamp(0.0, 255.0) as u8;
                        out.data[i + 1] = pg.round().clamp(0.0, 255.0) as u8;
                        out.data[i + 2] = pb.round().clamp(0.0, 255.0) as u8;
                        out.data[i + 3] = (pa * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
            out
        }

        let cases = [
            PanelSpec {
                w: 80,
                h: 48,
                radius: 10.0,
                top: [24, 30, 52, 232],
                bottom: [12, 16, 30, 240],
                horizontal: false,
                border: [180, 190, 220, 120],
                border_w: 1.5,
                shadow_blur: 14.0,
                shadow_dy: 4.0,
                shadow_alpha: 0.45,
                pad: 16,
                ramp: 0.0,
            },
            PanelSpec {
                w: 75,
                h: 41,
                radius: 8.0,
                top: [255, 200, 100, 0],
                bottom: [40, 80, 200, 210],
                horizontal: true,
                border: [255, 255, 255, 200],
                border_w: 2.0,
                shadow_blur: 10.0,
                shadow_dy: -3.0,
                shadow_alpha: 0.5,
                pad: 12,
                ramp: 30.0,
            },
            PanelSpec {
                w: 12,
                h: 10,
                radius: 5.0,
                top: [10, 20, 30, 255],
                bottom: [200, 150, 100, 128],
                horizontal: false,
                border: [255, 0, 0, 255],
                border_w: 3.0,
                shadow_blur: 0.0,
                shadow_dy: 0.0,
                shadow_alpha: 0.0,
                pad: 2,
                ramp: 0.0,
            },
        ];
        for (idx, s) in cases.iter().enumerate() {
            assert_eq!(render_panel(s).data, render_naive(s).data, "case {idx}");
        }
    }
}
