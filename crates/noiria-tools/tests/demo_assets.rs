//! `gen-demo-assets` が `games/demo/` へ生成した成果物の契約検査（寸法・チャンネル・立ち絵の体輪郭一致・音声の数値）。
//! 生成済みファイルを読む。再生成は `cargo run -p noiria-tools --bin gen-demo-assets -- games/demo/assets`。

use std::path::PathBuf;

use image::RgbaImage;
use noiria_tools::demo::chara::{render_sprites, Expr, Who};
use noiria_tools::demo::{bg, cg};

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../games/demo")
}

fn load(rel: &str) -> RgbaImage {
    let p = demo_dir().join(rel);
    image::open(&p)
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        .to_rgba8()
}

const EXPRS: [&str; 4] = ["normal", "smile", "blush", "sad"];

#[test]
fn backgrounds_and_cgs_are_opaque_1280x720() {
    for rel in [
        "assets/bg/street_morning.png",
        "assets/bg/classroom_evening.png",
        "assets/bg/clubroom.png",
        "assets/bg/rooftop_sunset.png",
        "assets/bg/hill_night.png",
        "assets/bg/title.png",
        "assets/cg/cg_meteor.png",
        "assets/cg/cg_hinata.png",
        "assets/cg/cg_shiori.png",
    ] {
        let img = load(rel);
        assert_eq!(img.dimensions(), (1280, 720), "{rel}");
        assert!(
            img.pixels().all(|p| p[3] == 255),
            "{rel}: 不透明でない画素あり"
        );
        let mean: f32 = img
            .pixels()
            .map(|p| (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0)
            .sum::<f32>()
            / (1280.0 * 720.0);
        assert!(mean > 8.0, "{rel}: ほぼ真っ黒 (mean={mean})");
    }
}

#[test]
fn title_left_half_is_calm() {
    let img = load("assets/bg/title.png");
    let mut sum = 0.0;
    let mut n = 0.0;
    for y in 0..720 {
        for x in 0..640 {
            let p = img.get_pixel(x, y);
            sum += 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
            n += 1.0;
        }
    }
    let mean = sum / n;
    assert!(mean < 90.0, "title 左半分が明るすぎる (mean luma={mean})");
}

#[test]
fn sprites_have_contract_canvas_and_matching_silhouette() {
    for who in ["hinata", "shiori"] {
        let imgs: Vec<RgbaImage> = EXPRS
            .iter()
            .map(|e| load(&format!("assets/fg/{who}_{e}.png")))
            .collect();
        for (i, img) in imgs.iter().enumerate() {
            let tag = format!("{who}_{}", EXPRS[i]);
            assert_eq!(img.dimensions(), (500, 900), "{tag}");
            let top = (0..900)
                .find(|&y| (0..500).any(|x| img.get_pixel(x, y)[3] > 128))
                .unwrap();
            assert!((50..=70).contains(&top), "{tag}: 頭頂 y={top} (期待 60 付近)");
            let bottom_opaque = (0..500).filter(|&x| img.get_pixel(x, 899)[3] == 255).count();
            assert!(bottom_opaque > 40, "{tag}: 下端まで体が続いていない");
            let transparent = img.pixels().filter(|p| p[3] == 0).count();
            assert!(transparent > 100_000, "{tag}: 透過領域が少なすぎる");
            let partial = img.pixels().filter(|p| p[3] > 0 && p[3] < 255).count();
            assert!(partial > 500, "{tag}: AA の半透明エッジが無い");
        }
        // 体・髪の輪郭（アルファ）は 4 表情でピクセル一致
        for (i, img) in imgs.iter().enumerate().skip(1) {
            let diff = img
                .pixels()
                .zip(imgs[0].pixels())
                .filter(|(a, b)| a[3] != b[3])
                .count();
            assert_eq!(diff, 0, "{who}_{}: normal と alpha が {diff} 画素異なる", EXPRS[i]);
        }
        // 首より下の RGBA も一致（差し替え時にガタつかない）
        for (i, img) in imgs.iter().enumerate().skip(1) {
            let mut diff_below = 0;
            for y in 340..900 {
                for x in 0..500 {
                    if img.get_pixel(x, y) != imgs[0].get_pixel(x, y) {
                        diff_below += 1;
                    }
                }
            }
            assert_eq!(
                diff_below, 0,
                "{who}_{}: 首より下の画素が normal と {diff_below} 個異なる",
                EXPRS[i]
            );
        }
        // 表情自体は差がある
        for (i, img) in imgs.iter().enumerate().skip(1) {
            let diff = img
                .pixels()
                .zip(imgs[0].pixels())
                .filter(|(a, b)| a != b)
                .count();
            assert!(diff > 300, "{who}_{}: normal との表情差が小さすぎる ({diff})", EXPRS[i]);
        }
    }
}

#[test]
fn sprites_have_no_dark_halo_on_translucent_edges() {
    // 半透明エッジが黒（透明の既定色）へ滲んでいないこと。輪郭線自体は暗色なので下限は緩く取る。
    for who in ["hinata", "shiori"] {
        let img = load(&format!("assets/fg/{who}_normal.png"));
        let mut sum = 0.0f64;
        let mut n = 0u32;
        for p in img.pixels() {
            if p[3] > 20 && p[3] < 235 {
                sum += (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0;
                n += 1;
            }
        }
        assert!(n > 500);
        let mean = sum / n as f64;
        assert!(mean > 20.0, "{who}: 半透明エッジが暗く滲んでいる (mean={mean})");
        // 完全透明画素の RGB は 0（無意味な色を持たない）
        assert!(img.pixels().filter(|p| p[3] == 0).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0));
    }
}

/// `(完全一致でない画素数, いずれかのチャンネルが ±1 を超えて違う画素数)`。
fn image_diff(disk: &RgbaImage, generated: &RgbaImage) -> (usize, usize) {
    assert_eq!(disk.dimensions(), generated.dimensions(), "寸法が違う");
    let (mut any, mut over) = (0usize, 0usize);
    for (a, b) in disk.pixels().zip(generated.pixels()) {
        let max_diff = (0..4).map(|c| a[c].abs_diff(b[c])).max().unwrap();
        any += (max_diff > 0) as usize;
        over += (max_diff > 1) as usize;
    }
    (any, over)
}

/// 生成物がコミット済み PNG と一致すること。`sin`/`cos`/`exp`/`powf` は OS の libm
/// （Windows は UCRT、Linux は glibc）でビット一致が保証されないため、チャンネルあたり ±1 の
/// 丸め差は許す。±1 を超える差は（生成ロジックが変わったので）失敗とする。
fn assert_matches_generator(label: &str, disk: &RgbaImage, generated: &RgbaImage) {
    let (any, over) = image_diff(disk, generated);
    assert_eq!(
        over, 0,
        "{label}: 生成済み PNG がジェネレータ出力と異なる（再生成してください）。\
         ±1 を超えて違う画素 {over} / 何らかの差がある画素 {any}"
    );
    if any > 0 {
        eprintln!("{label}: libm 由来とみられる ±1 の丸め差が {any} 画素ある（許容）");
    }
}

#[test]
fn image_diff_tolerates_one_and_flags_two() {
    let base = RgbaImage::from_pixel(4, 4, image::Rgba([10, 20, 30, 255]));
    let mut off_by_one = base.clone();
    off_by_one.put_pixel(1, 1, image::Rgba([11, 19, 30, 255]));
    assert_eq!(image_diff(&base, &off_by_one), (1, 0));
    let mut off_by_two = base.clone();
    off_by_two.put_pixel(2, 2, image::Rgba([10, 22, 30, 255]));
    assert_eq!(image_diff(&base, &off_by_two), (1, 1));
}

#[test]
fn committed_sprites_match_generator() {
    for (who, name) in [(Who::Hinata, "hinata"), (Who::Shiori, "shiori")] {
        for (e, img) in render_sprites(who) {
            let en = match e {
                Expr::Normal => "normal",
                Expr::Smile => "smile",
                Expr::Blush => "blush",
                Expr::Sad => "sad",
            };
            let disk = load(&format!("assets/fg/{name}_{en}.png"));
            assert_matches_generator(&format!("{name}_{en}"), &disk, &img);
        }
    }
}

/// 不透明画素のうち全体の 0.1% 以上を占める色の数。AA の境界色は 1 色あたりの画素が少なく数えない。
fn dominant_colors(img: &RgbaImage) -> usize {
    use std::collections::HashMap;
    let mut hist: HashMap<[u8; 3], u32> = HashMap::new();
    let mut opaque = 0u32;
    for p in img.pixels().filter(|p| p[3] == 255) {
        *hist.entry([p[0], p[1], p[2]]).or_default() += 1;
        opaque += 1;
    }
    hist.values().filter(|&&n| n * 1000 >= opaque).count()
}

#[test]
fn sprites_stay_flat_colored() {
    // フラット塗りの規約: グラデ・髪の筋・虹彩の階調などを描くと主要色が増える
    for who in ["hinata", "shiori"] {
        for e in EXPRS {
            let img = load(&format!("assets/fg/{who}_{e}.png"));
            let n = dominant_colors(&img);
            assert!((8..=DOMINANT_COLOR_MAX).contains(&n), "{who}_{e}: 主要色 {n} 色（上限 {DOMINANT_COLOR_MAX}）");
        }
    }
}

const DOMINANT_COLOR_MAX: usize = 20;

#[test]
fn event_cgs_keep_the_figure_small() {
    // 顔のアップは禁止。人物は画面高さの 1/3 以下・面積 3% 以下の引きの構図
    for (name, (_, meta)) in [("cg_hinata", cg::cg_hinata_with_meta()), ("cg_shiori", cg::cg_shiori_with_meta())] {
        let (x0, y0, x1, y1) = meta.bbox;
        assert!(y1 > y0 && x1 > x0, "{name}: 人物が描かれていない");
        assert!(y1 - y0 <= 720 / 3, "{name}: 人物の高さ {} px が画面の 1/3 を超える", y1 - y0);
        assert!(meta.area as f32 <= 1280.0 * 720.0 * 0.03, "{name}: 人物の面積 {} px が大きすぎる", meta.area);
    }
}

#[test]
fn committed_backgrounds_and_cgs_match_generator() {
    let (hinata, _) = cg::cg_hinata_with_meta();
    let (shiori, _) = cg::cg_shiori_with_meta();
    for (rel, img) in [
        ("assets/bg/street_morning.png", bg::street_morning()),
        ("assets/bg/classroom_evening.png", bg::classroom_evening()),
        ("assets/bg/clubroom.png", bg::clubroom()),
        ("assets/bg/rooftop_sunset.png", bg::rooftop_sunset()),
        ("assets/bg/hill_night.png", bg::hill_night()),
        ("assets/bg/title.png", bg::title_bg()),
        ("assets/cg/cg_meteor.png", bg::cg_meteor()),
        ("assets/cg/cg_hinata.png", hinata),
        ("assets/cg/cg_shiori.png", shiori),
    ] {
        assert_matches_generator(rel, &load(rel), &img);
    }
}

#[test]
fn rule_images_are_grayscale_1280x720() {
    for rel in [
        "assets/rule/wipe.png",
        "assets/rule/circle.png",
        "assets/rule/stars.png",
    ] {
        let img = load(rel);
        assert_eq!(img.dimensions(), (1280, 720), "{rel}");
        assert!(
            img.pixels()
                .all(|p| p[0] == p[1] && p[1] == p[2] && p[3] == 255),
            "{rel}: グレースケールでない"
        );
    }
    let stars = load("assets/rule/stars.png");
    let mut seen = [false; 256];
    for p in stars.pixels() {
        seen[p[0] as usize] = true;
    }
    let levels = seen.iter().filter(|&&b| b).count();
    assert!(levels >= 150, "stars.png の階調数が少ない ({levels})");
    let dark = stars.pixels().filter(|p| p[0] < 128).count();
    let bright = stars.pixels().filter(|p| p[0] > 240).count();
    assert!(
        dark > 20_000 && bright > 200_000,
        "stars.png が星形＋明るい地になっていない (dark={dark} bright={bright})"
    );
}

#[test]
fn icon_is_256_square_rgba() {
    // 自作 PNG エンコーダの出力を、別実装（image クレート）でデコードして確認する
    let img = load("icon.png");
    assert_eq!(img.dimensions(), (256, 256));
    assert!(img.pixels().any(|p| p[3] == 0) && img.pixels().any(|p| p[3] == 255));
}

fn read_wav(rel: &str) -> (hound::WavSpec, Vec<Vec<f32>>) {
    let p = demo_dir().join(rel);
    let mut r = hound::WavReader::open(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let spec = r.spec();
    let ch = spec.channels as usize;
    let mut chans = vec![Vec::new(); ch];
    for (i, s) in r.samples::<i16>().enumerate() {
        chans[i % ch].push(s.unwrap() as f32 / 32768.0);
    }
    (spec, chans)
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

#[test]
fn bgm_files_meet_loop_quality() {
    for name in ["title", "daily", "calm", "tension", "night"] {
        let (spec, ch) = read_wav(&format!("assets/bgm/{name}.wav"));
        assert_eq!(
            (spec.channels, spec.sample_rate, spec.bits_per_sample),
            (2, 44100, 16),
            "{name}"
        );
        let n = ch[0].len();
        let secs = n as f32 / 44100.0;
        assert!((28.0..=48.0).contains(&secs), "{name}: 長さ {secs}s");
        let peak = ch.iter().flatten().fold(0f32, |m, &v| m.max(v.abs()));
        assert!(peak <= 0.96, "{name}: ピーク {peak}");
        let both: Vec<f32> = ch[0].iter().chain(ch[1].iter()).copied().collect();
        let db = 20.0 * rms(&both).max(1e-9).log10();
        assert!((-20.5..=-11.5).contains(&db), "{name}: RMS {db} dBFS");
        for c in &ch {
            let dc = c.iter().sum::<f32>() / n as f32;
            assert!(dc.abs() < 2e-3, "{name}: DC {dc}");
            let w = 256;
            let mut seam: Vec<f32> = c[n - w..].to_vec();
            seam.extend_from_slice(&c[..w]);
            let d: f32 =
                seam.windows(2).map(|p| (p[1] - p[0]).abs()).sum::<f32>() / (seam.len() - 1) as f32;
            assert!(d < 0.02, "{name}: ループ境界不連続 {d}");
        }
        let (a, b) = (&ch[0], &ch[1]);
        let (ma, mb) = (a.iter().sum::<f32>() / n as f32, b.iter().sum::<f32>() / n as f32);
        let (mut cov, mut va, mut vb) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..n {
            cov += (a[i] - ma) * (b[i] - mb);
            va += (a[i] - ma).powi(2);
            vb += (b[i] - mb).powi(2);
        }
        assert!(cov / (va.sqrt() * vb.sqrt()) < 0.99, "{name}: ほぼモノラル");
    }
}

#[test]
fn se_files_are_mono_clean_and_faded() {
    for (name, lo, hi) in [
        ("click", 0.05, 1.0),
        ("chime", 2.5, 4.0),
        ("sparkle", 0.6, 1.2),
        ("heartbeat", 1.2, 2.0),
    ] {
        let (spec, ch) = read_wav(&format!("assets/se/{name}.wav"));
        assert_eq!(
            (spec.channels, spec.sample_rate, spec.bits_per_sample),
            (1, 44100, 16),
            "{name}"
        );
        let s = &ch[0];
        let secs = s.len() as f32 / 44100.0;
        assert!((lo..=hi).contains(&secs), "{name}: 長さ {secs}s");
        let peak = s.iter().fold(0f32, |m, &v| m.max(v.abs()));
        assert!(peak > 0.05 && peak <= 0.96, "{name}: ピーク {peak}");
        assert!((s.iter().sum::<f32>() / s.len() as f32).abs() < 1e-3, "{name}: DC");
        // click は既存 sample と共用（フェード無しの短い KS 音）。新規 SE は末尾が無音であること
        if name != "click" {
            assert!(s[s.len() - 1].abs() < 2e-3, "{name}: 末尾が無音になっていない");
        }
    }
}
