//! 夜の丘（hill_night）と、流星 CG・タイトルが共有する夜空・丘・木の部品。フラット塗り。

use super::*;

pub const HORIZON: f32 = 470.0;

/// 夜空: 縦 1 方向のグラデに、疎らな星と大きめの瞬き星だけを置く。
pub fn night_sky(cv: &mut Canvas, seed: u64, dots: usize, sparkles: usize) {
    vgrad(cv, 0.0, HORIZON + 40.0, col(14, 20, 58), col(66, 78, 142));
    let mut rng = Rng::new(seed);
    for _ in 0..dots {
        let (x, y) = (rng.r(10.0, 1270.0), rng.r(10.0, HORIZON - 70.0));
        let r = rng.r(1.4, 2.4);
        ell(cv, x, y, r, r, cola(232, 238, 255, rng.r(0.55, 0.95)));
    }
    for _ in 0..sparkles {
        let (x, y) = (rng.r(40.0, 1240.0), rng.r(30.0, HORIZON - 160.0));
        sparkle(cv, x, y, rng.r(8.0, 13.0), col(246, 248, 255));
    }
}

/// 天の川: 斜めの淡い帯を 2 枚重ねるだけ。
pub fn milky_way(cv: &mut Canvas) {
    poly(cv, &[(520.0, 0.0), (760.0, 0.0), (1010.0, 470.0), (700.0, 470.0)], cola(120, 134, 206, 0.16));
    poly(cv, &[(590.0, 0.0), (700.0, 0.0), (900.0, 470.0), (790.0, 470.0)], cola(150, 164, 224, 0.14));
}

fn tree(cv: &mut Canvas, x: f32, base: f32, c: Rgba32) {
    poly(
        cv,
        &[
            (x - 14.0, base + 6.0), (x - 8.0, base - 150.0), (x - 40.0, base - 230.0), (x - 30.0, base - 236.0),
            (x + 2.0, base - 180.0), (x + 12.0, base - 250.0), (x + 22.0, base - 246.0), (x + 12.0, base - 150.0),
            (x + 16.0, base + 6.0),
        ],
        c,
    );
    for (dx, dy, r) in [(0.0, -300.0, 84.0), (-70.0, -262.0, 56.0), (72.0, -268.0, 58.0), (-18.0, -352.0, 48.0), (46.0, -338.0, 44.0)] {
        ell(cv, x + dx, base + dy, r, r * 0.86, c);
    }
}

/// 丘 2 枚と街あかり少々、左の木。人物は丘の上に立たせる。
pub fn hills_and_tree(cv: &mut Canvas) {
    poly(
        cv,
        &[(0.0, 500.0), (200.0, 474.0), (480.0, 456.0), (760.0, 470.0), (1000.0, 450.0), (1280.0, 476.0), (1280.0, 720.0), (0.0, 720.0)],
        col(34, 44, 94),
    );
    for (x, y) in [(930.0, 474.0), (960.0, 470.0), (1012.0, 468.0), (1050.0, 466.0), (1104.0, 470.0), (1152.0, 474.0), (1200.0, 478.0)] {
        rect(cv, x, y, 9.0, 7.0, col(246, 208, 120));
    }
    poly(
        cv,
        &[(0.0, 566.0), (260.0, 512.0), (520.0, 504.0), (800.0, 540.0), (1280.0, 524.0), (1280.0, 720.0), (0.0, 720.0)],
        col(22, 30, 70),
    );
    tree(cv, 250.0, 516.0, col(13, 19, 46));
}

pub fn night_canvas() -> Canvas {
    let mut cv = canvas();
    night_sky(&mut cv, 11, 34, 7);
    milky_way(&mut cv);
    hills_and_tree(&mut cv);
    cv
}

/// 流れ星を空に足す。決定論（固定シード）。右上から左下へ流れる。
pub fn add_meteors(cv: &mut Canvas, seed: u64, count: usize) {
    let mut rng = Rng::new(seed ^ 0x6d65_7465);
    for _ in 0..count {
        let len = rng.r(180.0, 330.0);
        let ang = 2.05 + rng.r(-0.18, 0.18);
        let head = (rng.r(160.0, 1200.0), rng.r(60.0, 300.0));
        // 右上から左下へ流れるので、頭は進行側（左下）、尾は右上へ伸びる
        meteor(cv, head, (-ang.cos(), -ang.sin()), len, rng.r(1.8, 2.8));
    }
}
