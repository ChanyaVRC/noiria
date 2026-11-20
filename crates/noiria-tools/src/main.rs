//! gen-assets — サンプルゲーム用のライセンスフリーなプレースホルダアセットを手続き生成する。
//! 使い方: cargo run -p noiria-tools -- games/sample/assets

use std::f32::consts::PI;
use std::path::{Path, PathBuf};

use anyhow::Result;
use image::{Rgb, Rgba, RgbaImage};
use noiria_tools::{synth, wavout};
use noiria_tools::paint::{
    aa_bezier, aa_circle, aa_ellipse_soft, aa_line, aa_rounded_rect, aa_rounded_rect_stroke,
    add_radial_glow, blend_over, dither_noise, fbm_noise, fill_vertical_gradient, lerp, lerp_color,
    smoothstep01, soft_shadow_rect, vignette, Color,
};

const W: u32 = 1280;
const H: u32 = 720;

fn main() -> Result<()> {
    let out: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "games/sample/assets".to_string())
        .into();
    for sub in ["bg", "fg", "bgm", "se", "voice", "rule", "video"] {
        std::fs::create_dir_all(out.join(sub))?;
    }

    bg_room(&out.join("bg/room.png"))?;
    bg_street(&out.join("bg/street.png"))?;
    chara(&out.join("fg/alice_normal.png"), false)?;
    chara(&out.join("fg/alice_smile.png"), true)?;
    rule_wipe(&out.join("rule/wipe.png"))?;
    rule_circle(&out.join("rule/circle.png"))?;
    bgm(&out.join("bgm/main.wav"))?;
    bgm_alt_calm(&out.join("bgm/alt_calm.wav"))?;
    bgm_alt_tension(&out.join("bgm/alt_tension.wav"))?;
    se_click(&out.join("se/click.wav"))?;
    voice_beep(&out.join("voice/alice_001.wav"))?;
    video_demo(&out.join("video/demo.avi"))?;
    video_demo_mp4(&out.join("video/demo.mp4"));

    println!("アセットを生成しました: {}", out.display());
    Ok(())
}

/// 暖色の室内風グラデーション + 窓（外光グロー・立体窓枠）+ 板張りの床 + 家具シルエット
fn bg_room(path: &Path) -> Result<()> {
    let mut img = RgbaImage::new(W, H);
    let floor_top = 560.0f32;
    let baseboard_h = 14.0f32;

    // 壁: 上→下の垂直グラデーション + ディザ
    fill_vertical_gradient(&mut img, [140, 106, 84], [78, 58, 46]);

    // 壁の質感（微妙な明度ムラ、低周波 fBm ノイズ）。斑点感を避けるため振幅は控えめに。
    for y in 0..(floor_top as u32) {
        for x in 0..W {
            let n = fbm_noise(x as f32 * 0.022, y as f32 * 0.022) - 0.5;
            let px = img.get_pixel_mut(x, y);
            for c in px.0.iter_mut().take(3) {
                *c = (*c as f32 + n * 9.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    // 窓の外光グロー（壁に滲む暖色光）を窓本体より先に描く
    let win_x = 858.0f32;
    let win_y = 66.0f32;
    let win_w = 324.0f32;
    let win_h = 404.0f32;
    let win_cx = win_x + win_w / 2.0;
    let win_cy = win_y + win_h / 2.0;
    add_radial_glow(&mut img, win_cx, win_cy, 300.0, [255, 230, 190], 0.4);
    add_radial_glow(&mut img, win_cx, win_cy, 170.0, [255, 244, 214], 0.35);

    // 窓外枠（本体 + ベベル: 左上ハイライト / 右下シャドウ）
    let frame_color: Color = [96, 72, 54, 255];
    aa_rounded_rect(
        &mut img,
        win_x - 16.0,
        win_y - 16.0,
        win_w + 32.0,
        win_h + 32.0,
        10.0,
        frame_color,
    );
    aa_line(
        &mut img,
        win_x - 12.0,
        win_y - 10.0,
        win_x + win_w + 10.0,
        win_y - 10.0,
        3.0,
        [150, 118, 92, 255],
    );
    aa_line(
        &mut img,
        win_x - 12.0,
        win_y - 10.0,
        win_x - 12.0,
        win_y + win_h + 10.0,
        3.0,
        [150, 118, 92, 255],
    );
    aa_line(
        &mut img,
        win_x + win_w + 10.0,
        win_y - 8.0,
        win_x + win_w + 10.0,
        win_y + win_h + 12.0,
        3.0,
        [52, 38, 28, 255],
    );
    aa_line(
        &mut img,
        win_x - 10.0,
        win_y + win_h + 10.0,
        win_x + win_w + 12.0,
        win_y + win_h + 10.0,
        3.0,
        [52, 38, 28, 255],
    );

    // ガラス面: 夕空グラデーション
    for yy in 0..(win_h as i32) {
        let t = yy as f32 / win_h;
        let sky = lerp_color([210, 224, 240, 255], [156, 150, 190, 255], t);
        for xx in 0..(win_w as i32) {
            let n = dither_noise(xx as u32, yy as u32) * 0.5;
            let mut c = sky;
            for ch in c.iter_mut().take(3) {
                *ch = (*ch as f32 + n).round().clamp(0.0, 255.0) as u8;
            }
            img.put_pixel(
                (win_x as i32 + xx) as u32,
                (win_y as i32 + yy) as u32,
                Rgba(c),
            );
        }
    }
    add_radial_glow(
        &mut img,
        win_cx,
        win_y + win_h * 0.8,
        190.0,
        [255, 200, 140],
        0.24,
    );

    // 窓の桟（縦横）+ 細いハイライト
    aa_line(
        &mut img,
        win_cx,
        win_y + 4.0,
        win_cx,
        win_y + win_h - 4.0,
        9.0,
        frame_color,
    );
    aa_line(
        &mut img,
        win_x + 4.0,
        win_cy,
        win_x + win_w - 4.0,
        win_cy,
        9.0,
        frame_color,
    );
    aa_line(
        &mut img,
        win_cx - 3.0,
        win_y + 4.0,
        win_cx - 3.0,
        win_y + win_h - 4.0,
        1.6,
        [138, 108, 84, 255],
    );
    aa_line(
        &mut img,
        win_x + 4.0,
        win_cy - 3.0,
        win_x + win_w - 4.0,
        win_cy - 3.0,
        1.6,
        [138, 108, 84, 255],
    );
    // 内枠（影を落として奥行きを出す）
    aa_rounded_rect_stroke(
        &mut img,
        win_x,
        win_y,
        win_w,
        win_h,
        4.0,
        5.0,
        [40, 30, 22, 200],
    );

    // 幅木（壁と床の境界）
    aa_rounded_rect(
        &mut img,
        0.0,
        floor_top - baseboard_h,
        W as f32,
        baseboard_h,
        0.0,
        [54, 40, 32, 255],
    );
    aa_line(
        &mut img,
        0.0,
        floor_top - baseboard_h,
        W as f32,
        floor_top - baseboard_h,
        2.0,
        [96, 76, 60, 255],
    );

    // 床: 手前が明るい微グラデーション
    for y in (floor_top as u32)..H {
        let t = (y as f32 - floor_top) / (H as f32 - floor_top);
        let base = lerp_color([70, 50, 38, 255], [118, 86, 62, 255], t);
        for x in 0..W {
            let n = dither_noise(x, y);
            let mut c = base;
            for ch in c.iter_mut().take(3) {
                *ch = (*ch as f32 + n).round().clamp(0.0, 255.0) as u8;
            }
            img.put_pixel(x, y, Rgba(c));
        }
    }
    // 板目: パースで奥ほど間隔が詰まる横線
    for i in 1..=7 {
        let t = i as f32 / 8.0;
        let y = floor_top + (H as f32 - floor_top) * (t * t);
        let shade_t = t;
        let seam = lerp_color([40, 28, 20, 130], [30, 20, 14, 90], shade_t);
        aa_line(&mut img, 0.0, y, W as f32, y, 1.5, seam);
    }

    // 家具シルエット: 本棚（窓の光源=右奥からの陰影を左側面に付ける）
    draw_bookshelf(&mut img, 56.0, 300.0, 232.0, floor_top - baseboard_h);

    // 部屋の四隅ビネット
    vignette(&mut img, 0.2);

    img.save(path)?;
    Ok(())
}

/// 本棚シルエット（枠 + 棚板 + 本の背表紙 + サイドシェーディング + 落ち影）。
fn draw_bookshelf(img: &mut RgbaImage, x: f32, top: f32, w: f32, bottom: f32) {
    let h = bottom - top;
    soft_shadow_rect(
        img,
        x + 6.0,
        bottom - 6.0,
        w - 12.0,
        20.0,
        16.0,
        [8, 6, 5, 130],
    );

    let wood_dark: Color = [42, 30, 22, 255];
    let wood_light: Color = [70, 50, 36, 255];
    aa_rounded_rect(img, x, top, w, h, 6.0, wood_dark);
    aa_line(
        img,
        x + 4.0,
        top + 3.0,
        x + w - 4.0,
        top + 3.0,
        2.0,
        [96, 70, 50, 255],
    );

    // 光源は右側（窓側）想定: 左が暗く、右が明るいシェーディングを重ねる
    for yy in (top as i32)..(bottom as i32) {
        for xx in (x as i32)..((x + w) as i32) {
            let nx = (xx as f32 - x) / w; // 0(左)..1(右)
            let light = smoothstep01(nx) * 0.6;
            blend_over(img, xx, yy, wood_light, light * 0.35);
        }
    }

    // 棚板 3 段
    let shelves = 3;
    for i in 1..=shelves {
        let y = top + h * (i as f32) / (shelves as f32 + 1.0);
        aa_line(img, x + 3.0, y, x + w - 3.0, y, 4.0, wood_dark);
        aa_line(
            img,
            x + 3.0,
            y - 2.0,
            x + w - 3.0,
            y - 2.0,
            1.2,
            [90, 66, 48, 255],
        );

        // 本の背表紙（段ごとに数冊、簡易な色バリエーション）
        let book_colors: [Color; 5] = [
            [150, 60, 55, 255],
            [70, 100, 130, 255],
            [140, 120, 60, 255],
            [90, 120, 80, 255],
            [120, 80, 130, 255],
        ];
        let mut bx = x + 12.0;
        let shelf_bottom = y - 3.0;
        let mut seed = i * 7;
        while bx < x + w - 16.0 {
            let bw = 10.0 + (dither_noise(seed as u32, (i * 13) as u32).abs() * 10.0);
            let bh = 34.0 + (dither_noise((seed + 3) as u32, i as u32).abs() * 26.0);
            let color = book_colors[(seed as usize) % book_colors.len()];
            aa_rounded_rect(img, bx, shelf_bottom - bh, bw, bh, 1.5, color);
            bx += bw + 3.0;
            seed += 1;
        }
    }
    aa_rounded_rect_stroke(img, x, top, w, h, 6.0, 2.0, [20, 14, 10, 200]);
}

/// 夕暮れの街風グラデーション + 太陽グロー + 雲 + 大気遠近ビル群 + 電線
fn bg_street(path: &Path) -> Result<()> {
    let mut img = RgbaImage::new(W, H);
    let sky_top: [u8; 3] = [255, 160, 104];
    let sky_bottom: [u8; 3] = [56, 46, 84];
    fill_vertical_gradient(&mut img, sky_top, sky_bottom);

    // 太陽: 二重の放射グローと円盤
    let sun = (972.0f32, 232.0f32);
    add_radial_glow(&mut img, sun.0, sun.1, 430.0, [255, 200, 140], 0.16);
    add_radial_glow(&mut img, sun.0, sun.1, 240.0, [255, 214, 150], 0.5);
    aa_circle(&mut img, sun.0, sun.1, 46.0, [255, 236, 202, 255]);
    aa_circle(&mut img, sun.0, sun.1, 30.0, [255, 250, 232, 255]);

    // 雲: 横に伸びた柔らかい楕円の帯
    let clouds: [(f32, f32, f32, f32, u8); 3] = [
        (300.0, 100.0, 190.0, 22.0, 130),
        (760.0, 150.0, 240.0, 26.0, 110),
        (120.0, 210.0, 160.0, 18.0, 90),
    ];
    for (cx, cy, rx, ry, alpha) in clouds {
        aa_ellipse_soft(&mut img, cx, cy, rx, ry, [255, 214, 196, alpha], 0.75);
    }

    // ビル群: 遠いビルほど空色と混ぜて大気遠近感を出す
    let buildings: [(f32, f32, f32, f32); 8] = [
        (0.0, 380.0, 180.0, 0.30),
        (160.0, 300.0, 140.0, 0.55),
        (280.0, 420.0, 200.0, 0.10),
        (460.0, 340.0, 160.0, 0.28),
        (600.0, 260.0, 220.0, 0.60),
        (800.0, 400.0, 150.0, 0.18),
        (930.0, 320.0, 180.0, 0.42),
        (1090.0, 380.0, 190.0, 0.22),
    ];
    for (bi, (x, top, w, haze)) in buildings.into_iter().enumerate() {
        let sky_t = (top / H as f32).clamp(0.0, 1.0);
        let sky_here = [
            lerp(sky_top[0] as f32, sky_bottom[0] as f32, sky_t),
            lerp(sky_top[1] as f32, sky_bottom[1] as f32, sky_t),
            lerp(sky_top[2] as f32, sky_bottom[2] as f32, sky_t),
        ];
        let dark = [34.0, 28.0, 50.0];
        let base = [
            lerp(dark[0], sky_here[0], haze).round().clamp(0.0, 255.0) as u8,
            lerp(dark[1], sky_here[1], haze).round().clamp(0.0, 255.0) as u8,
            lerp(dark[2], sky_here[2], haze).round().clamp(0.0, 255.0) as u8,
        ];
        rect(
            &mut img,
            x as u32,
            top as u32,
            w as u32,
            H - top as u32,
            [base[0], base[1], base[2], 255],
        );

        // 窓明かり: 暖色/白の 2 色 + 小さなブルーム
        let mut yy = top as u32 + 24;
        while yy + 10 < H - 40 {
            let mut xx = x as u32 + 12;
            while xx + 8 < x as u32 + w as u32 - 8 {
                let n = dither_noise(xx + bi as u32 * 97, yy);
                if n > 0.35 {
                    let warm = n > 0.7;
                    let color: [u8; 3] = if warm {
                        [255, 214, 130]
                    } else {
                        [225, 235, 255]
                    };
                    rect(&mut img, xx, yy, 7, 9, [color[0], color[1], color[2], 255]);
                    add_radial_glow(&mut img, xx as f32 + 3.5, yy as f32 + 4.5, 7.0, color, 0.55);
                }
                xx += 22;
            }
            yy += 34;
        }
    }

    // 電線: たわみのある 2 本、ビル前面に描く
    aa_bezier(
        &mut img,
        (-20.0, 470.0),
        (640.0, 560.0),
        (1300.0, 490.0),
        1.6,
        [18, 16, 22, 220],
        48,
    );
    aa_bezier(
        &mut img,
        (-20.0, 500.0),
        (640.0, 600.0),
        (1300.0, 520.0),
        1.4,
        [18, 16, 22, 190],
        48,
    );

    rect(&mut img, 0, 640, W, H - 640, [25, 22, 40, 255]);
    aa_line(
        &mut img,
        0.0,
        640.0,
        W as f32,
        640.0,
        2.0,
        [70, 55, 60, 160],
    );

    vignette(&mut img, 0.18);

    img.save(path)?;
    Ok(())
}

/// 立ち絵シルエット (500x900, 透過)。最初期のめっちゃシンプルな図形キャラ（ユーザー指定で採用）。
/// smile で口が弧になる。
fn chara(path: &Path, smile: bool) -> Result<()> {
    let (w, h) = (500u32, 900u32);
    let mut img = RgbaImage::new(w, h);
    let body = [70, 90, 140, 255];
    let skin = [235, 205, 180, 255];
    let hair = [60, 45, 40, 255];
    let cx = w as i32 / 2;

    // 体（台形風）
    for y in 300..h as i32 {
        let t = (y - 300) as f32 / (h - 300) as f32;
        let half = lerp(90.0, 190.0, t) as i32;
        for x in (cx - half)..(cx + half) {
            img.put_pixel(x as u32, y as u32, Rgba(body));
        }
    }
    // 頭（円）
    circle(&mut img, cx, 190, 110, skin);
    // 髪（上半分の円 + サイド）
    for y in 60..190 {
        for x in (cx - 118)..(cx + 118) {
            let dx = (x - cx) as f32;
            let dy = (y - 170) as f32;
            if dx * dx + dy * dy < 118.0 * 118.0 {
                img.put_pixel(x as u32, y as u32, Rgba(hair));
            }
        }
    }
    rect_i(&mut img, cx - 118, 150, 26, 160, hair);
    rect_i(&mut img, cx + 92, 150, 26, 160, hair);
    // 目
    circle(&mut img, cx - 42, 200, 10, [40, 40, 45, 255]);
    circle(&mut img, cx + 42, 200, 10, [40, 40, 45, 255]);
    // 口
    if smile {
        for t in 0..60 {
            let th = std::f32::consts::PI * (t as f32) / 60.0;
            let x = cx + ((th.cos()) * -28.0) as i32;
            let y = 245 + ((th.sin()) * 14.0) as i32;
            circle(&mut img, x, y, 3, [170, 80, 80, 255]);
        }
    } else {
        rect_i(&mut img, cx - 22, 248, 44, 6, [170, 80, 80, 255]);
    }
    img.save(path)?;
    Ok(())
}

fn rect_i(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, c: [u8; 4]) {
    for yy in y.max(0)..(y + h).min(img.height() as i32) {
        for xx in x.max(0)..(x + w).min(img.width() as i32) {
            img.put_pixel(xx as u32, yy as u32, Rgba(c));
        }
    }
}

/// 横方向 0→255 のルール画像（ワイプ）
fn rule_wipe(path: &Path) -> Result<()> {
    let mut img = RgbaImage::new(W, H);
    for x in 0..W {
        let v = (x * 255 / (W - 1)) as u8;
        for y in 0..H {
            img.put_pixel(x, y, Rgba([v, v, v, 255]));
        }
    }
    img.save(path)?;
    Ok(())
}

/// 中心からの放射ルール画像
fn rule_circle(path: &Path) -> Result<()> {
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
    img.save(path)?;
    Ok(())
}

// ---- 動画 (MJPEG AVI) ----

/// 320x180, 12fps, 36 フレーム (~3秒)。グラデーション背景を丸が左→右へ横切る。
/// 自作 MJPEG デコーダの動作確認用サンプル動画。
fn video_demo(path: &Path) -> Result<()> {
    let (w, h) = (320u32, 180u32);
    let n_frames = 36u32;
    let fps = 12u32;

    let mut jpeg_frames = Vec::with_capacity(n_frames as usize);
    for i in 0..n_frames {
        let t = i as f32 / (n_frames - 1) as f32;
        let mut img = image::RgbImage::new(w, h);
        for y in 0..h {
            let ty = y as f32 / h as f32;
            for x in 0..w {
                let r = lerp(30.0, 150.0, ty) as u8;
                let g = lerp(50.0, 40.0, ty) as u8;
                let b = lerp(170.0, 70.0, ty) as u8;
                img.put_pixel(x, y, Rgb([r, g, b]));
            }
        }
        let cx = lerp(28.0, (w - 28) as f32, t) as i32;
        let cy = (h / 2) as i32;
        circle_rgb(&mut img, cx, cy, 20, [255, 220, 120]);

        let mut jpg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 85).encode(
            img.as_raw(),
            w,
            h,
            image::ExtendedColorType::Rgb8,
        )?;
        jpeg_frames.push(jpg);
    }

    let avi = noiria_core::codec::video::write_mjpeg_avi(w, h, fps, &jpeg_frames);
    std::fs::write(path, avi)?;
    Ok(())
}

// ---- 動画 (H.264 MP4, Media Foundation 経由) ----

/// video_demo と同じ絵柄 (320x180, 12fps, 36 フレーム) を H.264/MP4 で書き出す。
/// Windows 専用（Media Foundation の OS 内蔵エンコーダを使用）。エンコーダ MFT が
/// 無い環境では警告を出してスキップする（外部 ffmpeg 等は使わない方針のため）。
#[cfg(windows)]
fn video_demo_mp4(path: &Path) {
    let (w, h) = (320u32, 180u32);
    let n_frames = 36u32;
    let fps = 12u32;

    let mut rgba_frames = Vec::with_capacity(n_frames as usize);
    for i in 0..n_frames {
        let t = i as f32 / (n_frames - 1) as f32;
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            let ty = y as f32 / h as f32;
            let r = lerp(30.0, 150.0, ty) as u8;
            let g = lerp(50.0, 40.0, ty) as u8;
            let b = lerp(170.0, 70.0, ty) as u8;
            for x in 0..w {
                img.put_pixel(x, y, Rgba([r, g, b, 255]));
            }
        }
        let cx = lerp(28.0, (w - 28) as f32, t) as i32;
        let cy = (h / 2) as i32;
        circle(&mut img, cx, cy, 20, [255, 220, 120, 255]);
        rgba_frames.push(img.into_raw());
    }

    match noiria_core::codec::mf_video::encode_test_mp4(path, w, h, fps, &rgba_frames) {
        Ok(()) => println!("MP4 サンプルを生成しました: {}", path.display()),
        Err(e) => eprintln!(
            "警告: MP4 サンプル生成をスキップしました（H.264 エンコーダ MFT が利用できない環境の可能性）: {e:#}"
        ),
    }
}

/// Media Foundation は Windows 専用。他 OS ではサンプル生成をスキップする。
#[cfg(not(windows))]
fn video_demo_mp4(_path: &Path) {
    eprintln!("MP4 サンプル生成は Windows 専用のためスキップしました");
}

fn circle_rgb(img: &mut image::RgbImage, cx: i32, cy: i32, r: i32, c: [u8; 3]) {
    for y in (cy - r).max(0)..(cy + r).min(img.height() as i32) {
        for x in (cx - r).max(0)..(cx + r).min(img.width() as i32) {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r * r {
                img.put_pixel(x as u32, y as u32, Rgb(c));
            }
        }
    }
}

// ---- 音声 (WAV) ----

const SAMPLE_RATE: u32 = 44100;

fn wav_spec() -> hound::WavSpec {
    hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }
}

fn note(freq: f32, t: f32) -> f32 {
    // 単純なサイン + 2 倍音、アタック/リリース付き（voice_beep 用）
    (2.0 * PI * freq * t).sin() * 0.7 + (4.0 * PI * freq * t).sin() * 0.2
}

/// 温かい日常曲のステレオ BGM（手続き生成シンセ、シームレスループ）。
fn bgm(path: &Path) -> Result<()> {
    wavout::write_stereo(path, &synth::render_main())
}

/// 静かめの BGM（パッド + スローな KS メロディ中心）。
fn bgm_alt_calm(path: &Path) -> Result<()> {
    wavout::write_stereo(path, &synth::render_alt_calm())
}

/// 少し緊張感のある BGM（短調・低めベースのオスティナート + 疎なパッド）。
fn bgm_alt_tension(path: &Path) -> Result<()> {
    wavout::write_stereo(path, &synth::render_alt_tension())
}

/// クリック SE（短い KS プラック。以前の正弦ブリップより上品な減衰音）。
fn se_click(path: &Path) -> Result<()> {
    wavout::write_mono(path, &synth::render_click())
}

/// ボイス代わりの二音ビープ (~1.2 秒)
fn voice_beep(path: &Path) -> Result<()> {
    let mut writer = hound::WavWriter::create(path, wav_spec())?;
    for (freq, dur) in [(520.0f32, 0.5f32), (440.0, 0.7)] {
        let len = (dur * SAMPLE_RATE as f32) as usize;
        for i in 0..len {
            let t = i as f32 / SAMPLE_RATE as f32;
            let env = (PI * i as f32 / len as f32).sin() * 0.35;
            let s = note(freq, t) * env;
            writer.write_sample((s * i16::MAX as f32 * 0.8) as i16)?;
        }
    }
    writer.finalize()?;
    Ok(())
}

// ---- 描画ヘルパ ----

fn rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, c: [u8; 4]) {
    for yy in y..(y + h).min(img.height()) {
        for xx in x..(x + w).min(img.width()) {
            img.put_pixel(xx, yy, Rgba(c));
        }
    }
}

fn circle(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, c: [u8; 4]) {
    for y in (cy - r).max(0)..(cy + r).min(img.height() as i32) {
        for x in (cx - r).max(0)..(cx + r).min(img.width() as i32) {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r * r {
                img.put_pixel(x as u32, y as u32, Rgba(c));
            }
        }
    }
}
