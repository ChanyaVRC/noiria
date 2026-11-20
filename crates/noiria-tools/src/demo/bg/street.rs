//! 朝の通学路。奥へ伸びる道と桜並木。光は左から。人物の邪魔をしないよう彩度は低め。

use super::*;

const VP: P = (700.0, 380.0);

fn canopy(cv: &mut Canvas, x: f32, y: f32, r: f32) {
    let lobes = [(0.0, 0.0, 1.0), (-0.78, 0.22, 0.74), (0.78, 0.2, 0.76), (-0.32, -0.62, 0.7), (0.4, -0.56, 0.66), (0.0, 0.5, 0.66)];
    let mut all = Mask::empty();
    for (dx, dy, k) in lobes {
        let m = Mask::from_path(&Path::ellipse(x + dx * r, y + dy * r, r * k, r * k * 0.9, 0.0), (cv.w, cv.h));
        all = all.union(&m);
    }
    cv.fill_solid(&all, col(246, 202, 214));
    // 右下だけに 1 段のベタ影（光は左上）
    let moved = all.shifted((-0.16 * r) as i32, (-0.2 * r) as i32);
    cv.fill_solid(&all.sub(&moved), col(232, 170, 192));
}

fn tree(cv: &mut Canvas, base: P, s: f32) {
    let h = 330.0 * s;
    let tw = 46.0 * s;
    poly(cv, &[(base.0 - tw * 0.5, base.1), (base.0 - tw * 0.32, base.1 - h), (base.0 + tw * 0.32, base.1 - h), (base.0 + tw * 0.5, base.1)], col(122, 94, 98));
    poly(cv, &[(base.0 + tw * 0.1, base.1), (base.0 + tw * 0.14, base.1 - h), (base.0 + tw * 0.32, base.1 - h), (base.0 + tw * 0.5, base.1)], col(98, 74, 82));
    canopy(cv, base.0, base.1 - h - 70.0 * s, 175.0 * s);
}

pub fn draw() -> Canvas {
    let mut cv = canvas();
    vgrad(&mut cv, 0.0, 380.0, col(176, 214, 238), col(252, 240, 228));
    ell(&mut cv, 250.0, 150.0, 78.0, 78.0, col(255, 246, 220));
    // 遠景の街並み（2 段、ベタ）
    let far = col(206, 218, 232);
    let near = col(190, 205, 224);
    for (x0, x1, h) in [(0.0, 150.0, 70.0), (150.0, 250.0, 100.0), (250.0, 380.0, 60.0), (380.0, 470.0, 90.0), (860.0, 980.0, 80.0), (980.0, 1100.0, 110.0), (1100.0, 1280.0, 66.0)] {
        rect(&mut cv, x0, 380.0 - h, x1 - x0, h + 4.0, far);
    }
    for (x0, x1, h) in [(60.0, 200.0, 44.0), (300.0, 440.0, 50.0), (900.0, 1040.0, 52.0), (1040.0, 1200.0, 40.0)] {
        rect(&mut cv, x0, 380.0 - h, x1 - x0, h + 4.0, near);
    }
    // 歩道 + 道
    rect(&mut cv, 0.0, 380.0, 1280.0, 340.0, col(236, 226, 220));
    poly(&mut cv, &[(VP.0 - 55.0, VP.1), (VP.0 + 55.0, VP.1), (1560.0, 720.0), (-160.0, 720.0)], col(204, 206, 216));
    poly(&mut cv, &[(VP.0 - 55.0, VP.1), (VP.0 - 62.0, VP.1), (-250.0, 720.0), (-160.0, 720.0)], col(246, 240, 234));
    poly(&mut cv, &[(VP.0 + 55.0, VP.1), (VP.0 + 62.0, VP.1), (1650.0, 720.0), (1560.0, 720.0)], col(246, 240, 234));
    // 中央線（遠景側のみ。手前はメッセージ窓の下）
    for (t0, t1) in [(0.10f32, 0.2f32), (0.32, 0.5)] {
        let p = |t: f32, side: f32| (VP.0 + side * (3.0 + 22.0 * t), VP.1 + (720.0 - VP.1) * t);
        poly(&mut cv, &[p(t0, -1.0), p(t0, 1.0), p(t1, 1.0), p(t1, -1.0)], col(246, 244, 238));
    }
    // 桜並木: 遠い順
    for (i, s) in [0.2f32, 0.32, 0.5, 1.0].into_iter().enumerate() {
        let y = [405.0, 440.0, 520.0, 735.0][i];
        let dx = [150.0, 300.0, 450.0, 590.0][i];
        tree(&mut cv, (VP.0 - dx * 0.42 - 20.0 * s, y), s);
        tree(&mut cv, (VP.0 + dx * 0.42 + 20.0 * s, y), s);
    }
    // 花びら（大きめの楕円を数枚だけ）
    let mut rng = Rng::new(5);
    for _ in 0..10 {
        let (x, y) = (rng.r(20.0, 1260.0), rng.r(420.0, 520.0));
        if (x - VP.0).abs() < 130.0 {
            continue;
        }
        path(&mut cv, &Path::ellipse(x, y, 11.0, 4.5, rng.r(-0.5, 0.5)), col(248, 208, 220));
    }
    cv
}
