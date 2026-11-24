//! エンジン主要ホットパスのマイクロベンチ（再現可能な定量計測用）。
//! 実行: cargo run --release -p noiria-core --example bench

use std::hint::black_box;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use noiria_core::codec::{inflate, jpeg, png, vorbis};
use noiria_core::compose::{compose_frame, ComposeBufs};
use noiria_core::crypto;
use noiria_core::font::registry::FontRegistry;
use noiria_core::mixer::{BufferSource, Mixer, Resampler};
use noiria_core::pak;
use noiria_core::pixmap::{Pixmap, ScrollDir};
use noiria_core::scale::scale_bilinear;
use noiria_core::scene::{LayerKind, Page, Scene, TextContent, TextReveal};
use noiria_core::text::{TextEngine, TextSegment, TextStyle};
use noiria_core::transition::{TransMethod, Transition};
use noiria_core::uiraster::{render_panel, PanelSpec};

fn noise(w: u32, h: u32, seed: &mut u64, alpha: bool) -> Pixmap {
    let mut lcg = move || {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*seed >> 33) as u8
    };
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        rgba.push(lcg());
        rgba.push(lcg());
        rgba.push(lcg());
        rgba.push(if alpha { lcg() } else { 255 });
    }
    Pixmap::from_rgba_straight(w, h, &rgba)
}

fn bench_n(name: &str, warmup: usize, iters: usize, mut f: impl FnMut()) {
    for _ in 0..warmup {
        f();
    }
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    let per = t0.elapsed().as_secs_f64() * 1000.0 / iters as f64;
    println!("{name:<42} {per:8.3} ms");
}

fn bench_resolution(label: &str, w: u32, h: u32) {
    println!("\n=== Compositor / Pixmap ({label} {w}x{h}) ===");
    let mut seed = 7u64;
    let opaque = noise(w, h, &mut seed, false);
    let translucent = noise(w, h, &mut seed, true);
    let small = noise(w * 5 / 12, h, &mut seed, true);
    let mut dst = Pixmap::new(w, h);
    let a = noise(w, h, &mut seed, false);
    let b = noise(w, h, &mut seed, false);
    let wipe_rule: Vec<u8> = (0..h)
        .flat_map(|_| (0..w).map(move |x| ((x * 255) / w.max(1)) as u8))
        .collect();
    let grad_rule: Vec<u8> = (0..w * h).map(|i| (i % 256) as u8).collect();

    bench_n(&format!("blit_over opaque {label}"), 3, 60, || {
        dst.blit_over(black_box(&opaque), 0, 0, 255);
    });
    bench_n(&format!("blit_over translucent {label}"), 3, 60, || {
        dst.blit_over(black_box(&translucent), 0, 0, 255);
    });
    bench_n(&format!("blit_over opacity=128 {label}"), 3, 60, || {
        dst.blit_over(black_box(&opaque), 0, 0, 128);
    });
    bench_n(&format!("lerp_from (crossfade) {label}"), 3, 60, || {
        dst.lerp_from(black_box(&a), black_box(&b), 0.5);
    });
    bench_n(&format!("universal_from (wipe) {label}"), 3, 60, || {
        dst.universal_from(
            black_box(&a),
            black_box(&b),
            black_box(&wipe_rule),
            0.5,
            64.0,
        );
    });
    bench_n(
        &format!("universal_from (noise-rule) {label}"),
        3,
        60,
        || {
            dst.universal_from(
                black_box(&a),
                black_box(&b),
                black_box(&grad_rule),
                0.5,
                64.0,
            );
        },
    );
    bench_n(&format!("scroll_from {label}"), 3, 60, || {
        dst.scroll_from(black_box(&a), black_box(&b), ScrollDir::Left, 0.5);
    });
    let mut out32 = vec![0u32; (w * h) as usize];
    bench_n(&format!("to_bgrx_u32 {label}"), 3, 60, || {
        dst.to_bgrx_u32(black_box(&mut out32));
    });
    bench_n(&format!("resize_area {label}->336x189"), 2, 10, || {
        black_box(opaque.resize_area(336, 189));
    });

    // 4 layers + text scene
    let mut text_engine = TextEngine::new();
    let style = TextStyle {
        font_size: if h >= 1080 { 39.0 } else { 26.0 },
        box_w: w * 7 / 10,
        box_h: h / 4,
        ..Default::default()
    };
    let raster = Arc::new(text_engine.raster_segments(
        &[TextSegment {
            text: "今夜の星空はいつもよりずっと澄んでいて、遠くの丘の上からでも町の灯りがはっきり見えた。".into(),
            ..Default::default()
        }],
        &style,
    ));

    let mut scene = Scene::new(w, h);
    let bg = scene.alloc_layer(LayerKind::Image);
    let ch1 = scene.alloc_layer(LayerKind::Image);
    let ch2 = scene.alloc_layer(LayerKind::Image);
    let msg = scene.alloc_layer(LayerKind::Text);
    for (id, pm, x, z) in [
        (bg, opaque.clone(), 0, 0),
        (ch1, small.clone(), (w / 6) as i32, 10),
        (ch2, small.clone(), (w / 2) as i32, 11),
    ] {
        let l = scene.layer_mut(Page::Fore, id).unwrap();
        l.pixmap = Some(Arc::new(pm));
        l.x = x;
        l.z = z;
        l.visible = true;
    }
    {
        let mut box_pm = Pixmap::new(w - 80, h / 4);
        box_pm.fill_rounded_rect(0, 0, w - 80, h / 4, 14, [10, 12, 30, 195]);
        let l = scene.layer_mut(Page::Fore, msg).unwrap();
        l.pixmap = Some(Arc::new(box_pm));
        l.x = 40;
        l.y = (h * 3 / 4 - 20) as i32;
        l.z = 100;
        l.visible = true;
        l.text = Some(TextContent {
            raster,
            reveal: TextReveal {
                from_glyph: 0,
                start: None,
                cps: 0.0,
            },
        });
    }
    scene.backlay(None);

    let mut bufs = ComposeBufs::new(w, h);
    let mut frame = Pixmap::new(w, h);
    let now = Instant::now();
    bench_n(&format!("compose 4-layer static {label}"), 3, 60, || {
        compose_frame(&scene, now, &mut bufs, &mut frame);
    });
    bench_n(&format!("compose 4-layer fresh {label}"), 3, 40, || {
        let mut b2 = ComposeBufs::new(w, h);
        compose_frame(&scene, now, &mut b2, &mut frame);
    });
    let mut step = 0i32;
    bench_n(&format!("compose top-layer moving {label}"), 3, 60, || {
        step = (step + 1) & 1;
        scene.layer_mut(Page::Fore, msg).unwrap().x = 40 + step;
        compose_frame(&scene, now, &mut bufs, &mut frame);
    });
    scene.layer_mut(Page::Fore, msg).unwrap().x = 40;

    scene.transition = Some(Transition {
        method: TransMethod::Crossfade,
        start: now,
        duration: std::time::Duration::from_secs(100),
        skip: false,
    });
    bench_n(&format!("compose crossfade {label}"), 3, 60, || {
        compose_frame(&scene, Instant::now(), &mut bufs, &mut frame);
    });
    scene.transition = Some(Transition {
        method: TransMethod::Universal {
            rule: Arc::new(wipe_rule),
            vague: 64.0,
        },
        start: now,
        duration: std::time::Duration::from_secs(100),
        skip: false,
    });
    bench_n(&format!("compose universal wipe {label}"), 3, 60, || {
        compose_frame(&scene, Instant::now(), &mut bufs, &mut frame);
    });
}

fn main() {
    bench_resolution("720p", 1280, 720);
    bench_resolution("1080p", 1920, 1080);

    println!("\n=== Scaler (scale_bilinear) ===");
    let src32: Vec<u32> = (0..1280 * 720)
        .map(|i| (i as u32).wrapping_mul(2654435761))
        .collect();
    for (dw, dh) in [(1920usize, 1080usize), (2560, 1440), (3840, 2160)] {
        let mut dst = vec![0u32; dw * dh];
        bench_n(&format!("scale 720p->{dw}x{dh} (1T)"), 2, 15, || {
            scale_bilinear(&src32, 1280, 720, &mut dst, dw, dh, 1);
        });
        bench_n(&format!("scale 720p->{dw}x{dh} (4T)"), 2, 30, || {
            scale_bilinear(&src32, 1280, 720, &mut dst, dw, dh, 4);
        });
    }

    println!("\n=== UI Panel Rasterizer (uiraster) ===");
    let choice_panel = PanelSpec {
        w: 720,
        h: 64,
        radius: 12.0,
        top: [24, 30, 64, 206],
        bottom: [12, 16, 40, 222],
        horizontal: false,
        border: [255, 255, 255, 40],
        border_w: 1.5,
        shadow_blur: 16.0,
        shadow_dy: 6.0,
        shadow_alpha: 0.45,
        pad: 24,
        ramp: 0.0,
    };
    let dialog_panel = PanelSpec {
        w: 640,
        h: 320,
        radius: 16.0,
        top: [24, 30, 64, 230],
        bottom: [12, 16, 40, 240],
        horizontal: false,
        border: [255, 255, 255, 64],
        border_w: 1.5,
        shadow_blur: 24.0,
        shadow_dy: 8.0,
        shadow_alpha: 0.55,
        pad: 34,
        ramp: 0.0,
    };
    let band_panel = PanelSpec {
        w: 1920,
        h: 480,
        radius: 0.0,
        top: [6, 9, 24, 0],
        bottom: [6, 9, 24, 220],
        horizontal: false,
        border: [0, 0, 0, 0],
        border_w: 0.0,
        shadow_blur: 0.0,
        shadow_dy: 0.0,
        shadow_alpha: 0.0,
        pad: 0,
        ramp: 192.0,
    };
    bench_n("render_panel choice 720x64 (shadow)", 2, 20, || {
        black_box(render_panel(black_box(&choice_panel)));
    });
    bench_n("render_panel dialog 640x320 (shadow)", 2, 10, || {
        black_box(render_panel(black_box(&dialog_panel)));
    });
    bench_n("render_panel band 1920x480 (flat)", 2, 30, || {
        black_box(render_panel(black_box(&band_panel)));
    });

    println!("\n=== Font Registry & Text Engine ===");
    bench_n("FontRegistry::scan_system_fonts", 0, 1, || {
        black_box(FontRegistry::scan_system_fonts());
    });
    let mut te = TextEngine::new();
    let style_outline = TextStyle {
        font_size: 28.0,
        outline_width: 2,
        box_w: 880,
        box_h: 200,
        ..Default::default()
    };
    let style_plain = TextStyle {
        font_size: 24.0,
        outline_width: 0,
        box_w: 880,
        box_h: 200,
        ..Default::default()
    };
    let sample_segs = [TextSegment {
        text: "「流星群が降る夜に、三人で屋上から空を見上げたことを、私はきっとずっと忘れないと思う。」".into(),
        ..Default::default()
    }];
    bench_n("text raster_segments (cold, outline=2)", 0, 1, || {
        let mut fresh = TextEngine::new();
        black_box(fresh.raster_segments(&sample_segs, &style_outline));
    });
    // Warm up glyph cache
    let _ = te.raster_segments(&sample_segs, &style_outline);
    bench_n("text raster_segments (warm, outline=2)", 2, 40, || {
        black_box(te.raster_segments(&sample_segs, &style_outline));
    });
    let _ = te.raster_segments(&sample_segs, &style_plain);
    bench_n("text raster_segments (warm, outline=0)", 2, 60, || {
        black_box(te.raster_segments(&sample_segs, &style_plain));
    });

    println!("\n=== Codecs, Crypto & Pak ===");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if let Ok(bg_png) = std::fs::read(root.join("games/sample/assets/bg/room.png")) {
        bench_n("png::decode room.png (982KB)", 1, 10, || {
            black_box(png::decode(black_box(&bg_png)).unwrap());
        });
        let dec = png::decode(&bg_png).unwrap();
        let thumb =
            Pixmap::from_rgba_straight(dec.width, dec.height, &dec.rgba).resize_area(240, 135);
        bench_n("png::encode 240x135 thumb", 1, 10, || {
            black_box(png::encode(thumb.width, thumb.height, &thumb.data));
        });
        bench_n("png::encode 1280x720 frame", 0, 2, || {
            black_box(png::encode(dec.width, dec.height, &dec.rgba));
        });
        // Generate baseline JPEG from decoded room.png via image crate (dev-dep)
        let mut jpg_bytes = Vec::new();
        let rgb: Vec<u8> = dec
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg_bytes, 85)
            .encode(&rgb, dec.width, dec.height, image::ExtendedColorType::Rgb8)
            .unwrap();
        bench_n("jpeg::decode 1280x720 (Q85)", 1, 10, || {
            black_box(jpeg::decode(black_box(&jpg_bytes)).unwrap());
        });
    }
    if let Ok(rule_png) = std::fs::read(root.join("games/demo/assets/rule/stars.png")) {
        bench_n("png::decode stars.png (262KB)", 1, 15, || {
            black_box(png::decode(black_box(&rule_png)).unwrap());
        });
    }
    let mut crypto_buf = vec![0x5Au8; 4 * 1024 * 1024];
    let key = [0x42u8; 32];
    let nonce = [0x11u8; 12];
    bench_n("chacha20::apply_keystream (4 MiB)", 2, 20, || {
        crypto::apply_keystream(&key, &nonce, 0, black_box(&mut crypto_buf));
    });
    let compressed_payload =
        noiria_core::codec::deflate::deflate_compress(&vec![0xABu8; 512 * 1024]);
    bench_n("inflate (512 KiB compressed)", 2, 30, || {
        black_box(inflate::inflate(&compressed_payload).unwrap());
    });
    // Synthetic Pak build + read
    let mut entries = Vec::new();
    for i in 0..8 {
        entries.push((format!("asset_{i}.bin"), vec![(i * 31) as u8; 512 * 1024]));
    }
    let (pak_bytes, _) = pak::build(entries, &key);
    let opened = pak::Pak::open(pak_bytes, &key).unwrap();
    bench_n("pak::read 512KiB deflated+chacha20+crc", 2, 30, || {
        black_box(opened.read("asset_0.bin").unwrap());
    });

    println!("\n=== Audio (Vorbis & Mixer) ===");
    let ogg_path =
        root.join("crates/noiria-core/tests/vectors_audio/stereo1_kalmykia_anthem_trimmed.ogg");
    if let Ok(ogg_bytes) = std::fs::read(&ogg_path) {
        bench_n("vorbis decode stereo1.ogg", 1, 5, || {
            black_box(vorbis::decode(black_box(&ogg_bytes)).unwrap());
        });
    }
    let pcm: Vec<f32> = (0..44100 * 2)
        .map(|i| ((i as f32) * 0.01).sin() * 0.5)
        .collect();
    let mut mixer = Mixer::new(2, 48000);
    for ch in 0..4 {
        let src = Resampler::new(BufferSource::new(2, 44100, pcm.clone()), 48000);
        mixer.play(ch, Box::new(src), true, 0.5, 0);
    }
    let mut mix_out = vec![0.0f32; 4800 * 2]; // 100ms @ 48kHz stereo
    bench_n("mixer::mix_into (4 active+resamp 100ms)", 3, 100, || {
        mixer.mix_into(black_box(&mut mix_out));
    });
}
