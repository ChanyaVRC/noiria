//! gen-demo-assets — 恋愛 ADV デモ「星待ちの放課後」用アセットの手続き生成。
//! 外部素材ゼロ。使い方: cargo run -p noiria-tools --bin gen-demo-assets -- games/demo/assets
//! `games/demo/icon.png` は出力先の 1 つ上（games/demo/）へ書く。

use std::path::{Path, PathBuf};

use anyhow::Result;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ImageEncoder, Rgba, RgbaImage};
use noiria_tools::demo::chara::{render_sprites_styled, Expr, Style, Who};
use noiria_tools::demo::{bg, cg, preview};
use noiria_tools::synth;
use noiria_tools::wavout;

const W: u32 = 1280;
const H: u32 = 720;

fn save_png(img: &RgbaImage, path: &Path) -> Result<()> {
    let mut f = std::fs::File::create(path)?;
    PngEncoder::new_with_quality(&mut f, CompressionType::Best, FilterType::Adaptive).write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(())
}

/// 横方向 0→255 のルール画像（gen-assets と同一の生成）。
fn rule_wipe() -> RgbaImage {
    let mut img = RgbaImage::new(W, H);
    for x in 0..W {
        let v = (x * 255 / (W - 1)) as u8;
        for y in 0..H {
            img.put_pixel(x, y, Rgba([v, v, v, 255]));
        }
    }
    img
}

/// 中心からの放射ルール画像（gen-assets と同一の生成）。
fn rule_circle() -> RgbaImage {
    let mut img = RgbaImage::new(W, H);
    let maxd = ((W * W / 4 + H * H / 4) as f32).sqrt();
    for y in 0..H {
        for x in 0..W {
            let dx = x as f32 - W as f32 / 2.0;
            let dy = y as f32 - H as f32 / 2.0;
            let v = ((dx * dx + dy * dy).sqrt() / maxd * 255.0).min(255.0) as u8;
            img.put_pixel(x, y, Rgba([v, v, v, 255]));
        }
    }
    img
}

/// 目視確認用の合成画像を `dir` へ書く（`--preview`）。アセットは書かない。
fn write_previews(dir: &Path, style: Style) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let classroom = bg::classroom_evening();
    save_png(&preview::style_candidates(&classroom), &dir.join("style_candidates.png"))?;
    save_png(&preview::sheet(Who::Hinata, style), &dir.join("sheet_hinata.png"))?;
    save_png(&preview::sheet(Who::Shiori, style), &dir.join("sheet_shiori.png"))?;
    let two = [(Who::Hinata, Expr::Smile, 140), (Who::Shiori, Expr::Normal, 640)];
    save_png(&preview::scene(&classroom, style, &two), &dir.join("classroom_two.png"))?;
    save_png(&preview::scene(&classroom, style, &[(Who::Hinata, Expr::Normal, 390)]), &dir.join("classroom_hinata_center.png"))?;
    save_png(&preview::scene(&classroom, style, &[(Who::Shiori, Expr::Blush, 390)]), &dir.join("classroom_shiori_center.png"))?;
    for (name, img) in [
        ("street_morning", bg::street_morning()),
        ("clubroom", bg::clubroom()),
        ("rooftop_sunset", bg::rooftop_sunset()),
        ("hill_night", bg::hill_night()),
        ("title", bg::title_bg()),
    ] {
        save_png(&preview::scene(&img, style, &two), &dir.join(format!("bg_{name}.png")))?;
    }
    save_png(&cg::cg_hinata(), &dir.join("cg_hinata.png"))?;
    save_png(&cg::cg_shiori(), &dir.join("cg_shiori.png"))?;
    save_png(&bg::cg_meteor(), &dir.join("cg_meteor.png"))?;
    println!("プレビューを書き出しました: {}", dir.display());
    Ok(())
}

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut style = Style::A;
    let mut preview_dir: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--style" | "--preview" => {
                let v = args.get(i + 1).cloned().unwrap_or_default();
                if args[i] == "--style" {
                    style = Style::parse(&v).ok_or_else(|| anyhow::anyhow!("--style は a|b|c: {v}"))?;
                } else {
                    preview_dir = Some(v.into());
                }
                args.drain(i..(i + 2).min(args.len()));
            }
            _ => i += 1,
        }
    }
    if let Some(d) = preview_dir {
        return write_previews(&d, style);
    }
    let out: PathBuf = args.first().cloned().unwrap_or_else(|| "games/demo/assets".into()).into();
    for sub in ["bg", "cg", "fg", "rule", "bgm", "se"] {
        std::fs::create_dir_all(out.join(sub))?;
    }

    for (name, img) in [
        ("street_morning", bg::street_morning()),
        ("classroom_evening", bg::classroom_evening()),
        ("clubroom", bg::clubroom()),
        ("rooftop_sunset", bg::rooftop_sunset()),
        ("hill_night", bg::hill_night()),
        ("title", bg::title_bg()),
    ] {
        save_png(&img, &out.join(format!("bg/{name}.png")))?;
    }
    save_png(&bg::cg_meteor(), &out.join("cg/cg_meteor.png"))?;
    save_png(&cg::cg_hinata(), &out.join("cg/cg_hinata.png"))?;
    save_png(&cg::cg_shiori(), &out.join("cg/cg_shiori.png"))?;

    for (who, name) in [(Who::Hinata, "hinata"), (Who::Shiori, "shiori")] {
        for (e, img) in render_sprites_styled(who, style) {
            let en = match e {
                Expr::Normal => "normal",
                Expr::Smile => "smile",
                Expr::Blush => "blush",
                Expr::Sad => "sad",
            };
            save_png(&img, &out.join(format!("fg/{name}_{en}.png")))?;
        }
    }

    save_png(&rule_wipe(), &out.join("rule/wipe.png"))?;
    save_png(&rule_circle(), &out.join("rule/circle.png"))?;
    save_png(&bg::rule_stars(), &out.join("rule/stars.png"))?;

    wavout::write_stereo(&out.join("bgm/title.wav"), &synth::demo::render_title())?;
    wavout::write_stereo(&out.join("bgm/daily.wav"), &synth::demo::render_daily())?;
    wavout::write_stereo(&out.join("bgm/calm.wav"), &synth::demo::render_calm())?;
    wavout::write_stereo(&out.join("bgm/tension.wav"), &synth::demo::render_tension())?;
    wavout::write_stereo(&out.join("bgm/night.wav"), &synth::demo::render_night())?;

    wavout::write_mono(&out.join("se/click.wav"), &synth::render_click())?;
    wavout::write_mono(&out.join("se/chime.wav"), &synth::demo::render_chime())?;
    wavout::write_mono(&out.join("se/sparkle.wav"), &synth::demo::render_sparkle())?;
    wavout::write_mono(&out.join("se/heartbeat.wav"), &synth::demo::render_heartbeat())?;

    // アイコンは sample と同じく自作 PNG エンコーダで書く（games/demo/icon.png）
    let icon = bg::icon();
    let png = noiria_core::codec::png::encode(icon.width(), icon.height(), icon.as_raw());
    let icon_path = out.parent().map(|p| p.join("icon.png")).unwrap_or_else(|| PathBuf::from("icon.png"));
    std::fs::write(&icon_path, png)?;

    println!("デモアセットを生成しました: {}", out.display());
    Ok(())
}
