//! デモ「星待ちの放課後」用の背景画像の手続き生成。外部素材なし、すべて決定論。
//!
//! 立ち絵と同じフラット塗り（限定パレット・大きな形・グラデは空の縦 1 方向のみ）。
//! 光源方向と遠近を各シーンで一貫させる。各シーンは `bg/` 配下の 1 ファイルが 1 枚の絵を描く。

#![allow(clippy::too_many_arguments)]

use super::gfx::*;
use image::RgbaImage;

mod cg;
mod classroom;
mod clubroom;
mod hill;
mod icon;
mod rooftop;
mod rule;
mod street;
mod title;
mod util;

use util::*;

/// 隣接ポリゴンの AA 継ぎ目に残る α<1 を 255 へ揃える（RGB は非プリマルチ済みなので色は変わらない）。
fn opaque(mut img: RgbaImage) -> RgbaImage {
    for p in img.pixels_mut() {
        p[3] = 255;
    }
    img
}

/// 朝の通学路（桜並木）。
pub fn street_morning() -> RgbaImage {
    opaque(street::draw().to_image(false))
}

/// 夕方の教室。
pub fn classroom_evening() -> RgbaImage {
    opaque(classroom::draw().to_image(false))
}

/// 天文部の部室。
pub fn clubroom() -> RgbaImage {
    opaque(clubroom::draw().to_image(false))
}

/// 夕焼けの屋上。
pub fn rooftop_sunset() -> RgbaImage {
    opaque(rooftop::draw(false).to_image(true))
}

/// 夜の丘。
pub fn hill_night() -> RgbaImage {
    opaque(hill_night_canvas().to_image(true))
}

/// タイトル背景（左半分は静かに）。
pub fn title_bg() -> RgbaImage {
    opaque(title::draw().to_image(true))
}

/// 流星群を見上げる 3 人の CG 背景。
pub fn cg_meteor() -> RgbaImage {
    opaque(cg::draw().to_image(true))
}

/// トランジション用ルール画像（グレースケール）。
pub fn rule_stars() -> RgbaImage {
    rule::draw()
}

/// アイコン（256x256）。
pub fn icon() -> RgbaImage {
    icon::draw()
}

/// `hill_night` の素の Canvas。
pub fn hill_night_canvas() -> Canvas {
    hill::night_canvas()
}

/// 屋上の夜版。
pub fn rooftop_night_canvas() -> Canvas {
    rooftop::draw(true)
}

/// 屋上の床の上端（人物の足元の目安）。
pub const ROOFTOP_FLOOR_Y: f32 = rooftop::FLOOR_Y;

/// 流れ星（尾の長いベタの線）を空に足す。固定シードで決定論。
pub fn add_meteors(cv: &mut Canvas, seed: u64, count: usize) {
    hill::add_meteors(cv, seed, count);
}
