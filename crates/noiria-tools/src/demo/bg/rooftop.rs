//! 屋上。夕焼け版は右に低い太陽、夜版（CG 用）は星空。フェンス越しに街を見る。光は右から（影は左へ伸びる）。

use super::*;

pub const FLOOR_Y: f32 = 520.0;

pub fn draw(night: bool) -> Canvas {
    let mut cv = canvas();
    let (sky_top, sky_mid, sky_low) = if night {
        (col(14, 20, 58), col(40, 50, 108), col(68, 80, 146))
    } else {
        (col(120, 106, 172), col(238, 152, 136), col(255, 208, 152))
    };
    cv.fill_all(|_, y| {
        if y < 300.0 {
            mix(sky_top, sky_mid, y / 300.0)
        } else {
            mix(sky_mid, sky_low, (y - 300.0) / 170.0)
        }
    });
    if night {
        let mut rng = Rng::new(31);
        for _ in 0..30 {
            let (x, y) = (rng.r(10.0, 1270.0), rng.r(10.0, 330.0));
            ell(
                &mut cv,
                x,
                y,
                rng.r(1.4, 2.4),
                rng.r(1.4, 2.4),
                cola(232, 238, 255, rng.r(0.55, 0.95)),
            );
        }
        for _ in 0..5 {
            let (x, y) = (rng.r(40.0, 1240.0), rng.r(30.0, 260.0));
            sparkle(&mut cv, x, y, rng.r(8.0, 13.0), col(246, 248, 255));
        }
    } else {
        ell(&mut cv, 930.0, 392.0, 70.0, 70.0, col(255, 238, 196));
        poly(
            &mut cv,
            &[
                (700.0, 330.0),
                (1180.0, 330.0),
                (1280.0, 360.0),
                (600.0, 360.0),
            ],
            cola(255, 214, 160, 0.25),
        );
    }
    // 遠景の街（2 段）
    let (far, near, win) = if night {
        (col(40, 50, 104), col(28, 36, 84), col(246, 206, 120))
    } else {
        (col(190, 134, 158), col(156, 106, 140), col(255, 226, 170))
    };
    let blds = [
        (0.0, 90.0, 70.0),
        (90.0, 190.0, 120.0),
        (190.0, 260.0, 86.0),
        (260.0, 390.0, 54.0),
        (700.0, 800.0, 62.0),
        (800.0, 920.0, 104.0),
        (1010.0, 1100.0, 78.0),
        (1100.0, 1280.0, 130.0),
    ];
    for (x0, x1, h) in blds {
        rect(&mut cv, x0, 470.0 - h, x1 - x0, h + 2.0, far);
    }
    for (x0, x1, h) in [
        (60.0, 180.0, 40.0),
        (330.0, 520.0, 56.0),
        (560.0, 720.0, 44.0),
        (900.0, 1040.0, 48.0),
    ] {
        rect(&mut cv, x0, 470.0 - h, x1 - x0, h + 2.0, near);
    }
    if night {
        for (x, y) in [
            (110.0, 380.0),
            (130.0, 410.0),
            (820.0, 380.0),
            (850.0, 420.0),
            (1130.0, 352.0),
            (1160.0, 392.0),
            (1200.0, 372.0),
        ] {
            rect(&mut cv, x, y, 10.0, 8.0, win);
        }
    }
    // 床と腰壁
    let (floor, wall_top, wall_face) = if night {
        (col(66, 70, 114), col(98, 104, 156), col(52, 56, 98))
    } else {
        (col(188, 160, 168), col(232, 204, 196), col(164, 134, 148))
    };
    rect(&mut cv, 0.0, FLOOR_Y, 1280.0, 200.0, floor);
    rect(&mut cv, 0.0, 468.0, 1280.0, 14.0, wall_top);
    rect(&mut cv, 0.0, 482.0, 1280.0, FLOOR_Y - 482.0, wall_face);
    // フェンスの影（夕方: 右の太陽から左下へ）
    if !night {
        for i in 0..8 {
            let x = 120.0 + i as f32 * 180.0;
            poly(
                &mut cv,
                &[
                    (x - 5.0, FLOOR_Y),
                    (x + 5.0, FLOOR_Y),
                    (x - 300.0, 720.0),
                    (x - 330.0, 720.0),
                ],
                cola(120, 84, 110, 0.28),
            );
        }
    }
    // フェンス: 上下の横木 + 支柱 + 斜め金網を 1 色の太線で
    let fence = if night {
        col(24, 30, 70)
    } else {
        col(104, 80, 108)
    };
    bar(&mut cv, (-10.0, 300.0), (1290.0, 300.0), 8.0, fence);
    bar(&mut cv, (-10.0, 468.0), (1290.0, 468.0), 8.0, fence);
    for i in 0..9 {
        let x = 40.0 + i as f32 * 160.0;
        bar(&mut cv, (x, 296.0), (x, 480.0), 9.0, fence);
    }
    // 給水タンク（左奥）
    let (tank, tank_lit) = if night {
        (col(60, 66, 112), col(78, 86, 134))
    } else {
        (col(150, 124, 148), col(194, 168, 176))
    };
    bar(&mut cv, (74.0, 400.0), (64.0, 470.0), 8.0, fence);
    bar(&mut cv, (196.0, 400.0), (206.0, 470.0), 8.0, fence);
    let body = rect(&mut cv, 50.0, 250.0, 170.0, 160.0, tank);
    cv.fill_solid(&body.sub(&body.shifted(-24, 0)), tank_lit);
    ell(&mut cv, 135.0, 250.0, 85.0, 16.0, tank_lit);
    cv
}
