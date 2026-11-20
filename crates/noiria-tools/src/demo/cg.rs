//! イベント CG（ひなた / しおり）。顔のアップは描かず、流星群の空を主役にした引きの構図に、
//! 立ち絵と同じ描画で縮小した全身を小さく立たせる（画面高さの 1/3 以下）。

use image::RgbaImage;

use super::bg::{self, add_meteors, hill_night_canvas, rooftop_night_canvas};
use super::chara::{draw_base, draw_expr, Expr, Opts, Pose, Style, Who};
use super::gfx::*;

/// CG 内の人物が占める領域（スタイル規約のテスト用）。
#[derive(Clone, Copy, Debug)]
pub struct FigureMeta {
    /// (x0, y0, x1, y1) 半開区間。alpha > 0.5 の画素の外接矩形。
    pub bbox: (u32, u32, u32, u32),
    /// alpha > 0.5 の画素数。
    pub area: u32,
}

/// 足元 `feet`（画面座標）に、スケール `s` で全身を立てる。夜の色調を乗せて背景へ合成する。
fn place_figure(bg: &mut Canvas, who: Who, expr: Expr, opts: Opts, feet: (f32, f32), s: f32, tint: [f32; 3]) -> FigureMeta {
    // 設計座標の足元 (250, 960) が `feet` に来るよう平行移動する
    let xf = Xf { s, rot: 0.0, tx: feet.0 - 250.0, ty: feet.1 - 960.0, cx: 250.0, cy: 960.0 };
    let mut layer = Canvas::new(bg.w, bg.h);
    {
        let mut ctx = Ctx::new(&mut layer, xf);
        let parts = draw_base(&mut ctx, who, Style::A, opts);
        draw_expr(&mut ctx, who, Style::A, opts, expr, &parts);
    }
    let (mut x0, mut y0, mut x1, mut y1, mut area) = (u32::MAX, u32::MAX, 0u32, 0u32, 0u32);
    for y in 0..layer.h {
        for x in 0..layer.w {
            let p = &mut layer.px[y * layer.w + x];
            if p[3] <= 0.0 {
                continue;
            }
            if p[3] > 0.5 {
                x0 = x0.min(x as u32);
                y0 = y0.min(y as u32);
                x1 = x1.max(x as u32 + 1);
                y1 = y1.max(y as u32 + 1);
                area += 1;
            }
            for c in 0..3 {
                p[c] *= tint[c];
            }
            let k = 1.0 - p[3];
            let d = &mut bg.px[y * bg.w + x];
            for c in 0..4 {
                d[c] = p[c] + d[c] * k;
            }
        }
    }
    FigureMeta { bbox: (x0, y0, x1, y1), area }
}

const FIGURE_SCALE: f32 = 0.26;
const NIGHT_TINT: [f32; 3] = [0.62, 0.68, 0.96];

/// 夜の丘で、木の横に立ち空を指すひなた。
pub fn cg_hinata_with_meta() -> (RgbaImage, FigureMeta) {
    let mut cv = hill_night_canvas();
    add_meteors(&mut cv, 7, 9);
    let opts = Opts { pose: Pose::Sky, head_ang: Some(-0.2) };
    let meta = place_figure(&mut cv, Who::Hinata, Expr::Smile, opts, (400.0, 552.0), FIGURE_SCALE, NIGHT_TINT);
    (cv.to_image(true), meta)
}

pub fn cg_hinata() -> RgbaImage {
    cg_hinata_with_meta().0
}

/// 屋上の夜、望遠鏡の隣で空を見上げるしおり。
pub fn cg_shiori_with_meta() -> (RgbaImage, FigureMeta) {
    let mut cv = rooftop_night_canvas();
    add_meteors(&mut cv, 11, 9);
    // 望遠鏡（三脚 + 鏡筒。空へ向ける）
    let steel = col(120, 130, 178);
    let dark = col(70, 78, 126);
    let hub = (872.0, 520.0);
    for foot in [(826.0, bg::ROOFTOP_FLOOR_Y + 90.0), (918.0, bg::ROOFTOP_FLOOR_Y + 90.0), (872.0, bg::ROOFTOP_FLOOR_Y + 100.0)] {
        let m = Mask::from_path(&Path::stroke(&[(hub.0, hub.1, 7.0), (foot.0, foot.1, 7.0)]), (cv.w, cv.h));
        cv.fill_solid(&m, dark);
    }
    for (a, b, w, c) in [((872.0, 520.0), (872.0, 484.0), 12.0, dark), ((862.0, 494.0), (948.0, 372.0), 30.0, steel), ((944.0, 378.0), (956.0, 360.0), 38.0, dark)] {
        let m = Mask::from_path(&Path::stroke(&[(a.0, a.1, w), (b.0, b.1, w)]), (cv.w, cv.h));
        cv.fill_solid(&m, c);
    }
    let opts = Opts { pose: Pose::Stand, head_ang: Some(-0.12) };
    let meta = place_figure(&mut cv, Who::Shiori, Expr::Blush, opts, (760.0, bg::ROOFTOP_FLOOR_Y + 88.0), FIGURE_SCALE, NIGHT_TINT);
    (cv.to_image(true), meta)
}

pub fn cg_shiori() -> RgbaImage {
    cg_shiori_with_meta().0
}
