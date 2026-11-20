//! 流星群を見上げる 3 人（ひなた・蒼太・しおり）の後ろ姿。夜の丘に、フラットな単純図形で。

use super::*;

struct Back {
    x: f32,
    by: f32,
    hair: Rgba32,
    top: Rgba32,
    long_hair: bool,
    bob: bool,
}

fn figure(cv: &mut Canvas, b: &Back) {
    let skin = col(204, 176, 168);
    let body = Path::closed_spline(&[
        (b.x - 34.0, b.by - 130.0), (b.x + 34.0, b.by - 130.0), (b.x + 66.0, b.by - 102.0), (b.x + 74.0, b.by + 40.0),
        (b.x - 74.0, b.by + 40.0), (b.x - 66.0, b.by - 102.0),
    ]);
    path(cv, &body, b.top);
    rect(cv, b.x - 12.0, b.by - 154.0, 24.0, 30.0, skin);
    if b.long_hair {
        poly(cv, &[(b.x - 46.0, b.by - 200.0), (b.x + 46.0, b.by - 200.0), (b.x + 54.0, b.by - 70.0), (b.x - 54.0, b.by - 70.0)], b.hair);
    }
    let (rx, ry) = if b.bob { (54.0, 54.0) } else { (44.0, 50.0) };
    ell(cv, b.x, b.by - 196.0, rx, ry, b.hair);
    if b.bob {
        poly(cv, &[(b.x - 54.0, b.by - 196.0), (b.x + 54.0, b.by - 196.0), (b.x + 50.0, b.by - 150.0), (b.x - 50.0, b.by - 150.0)], b.hair);
        bar(cv, (b.x + 20.0, b.by - 230.0), (b.x + 30.0, b.by - 206.0), 7.0, col(206, 168, 70));
    }
}

pub fn draw() -> Canvas {
    let mut cv = hill::night_canvas();
    hill::add_meteors(&mut cv, 7, 6);
    let people = [
        Back { x: 520.0, by: 706.0, hair: col(132, 86, 60), top: col(196, 202, 226), long_hair: false, bob: true },
        Back { x: 790.0, by: 708.0, hair: col(30, 34, 62), top: col(172, 154, 126), long_hair: true, bob: false },
        Back { x: 655.0, by: 692.0, hair: col(34, 36, 50), top: col(54, 62, 104), long_hair: false, bob: false },
    ];
    // 中央の蒼太を先に描き、両脇を肩が触れる位置に重ねる
    figure(&mut cv, &people[2]);
    figure(&mut cv, &people[0]);
    figure(&mut cv, &people[1]);
    cv
}
