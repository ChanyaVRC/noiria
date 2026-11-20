//! 合成ホットパスのマイクロベンチ（外部依存なし）。
//! 実行: cargo run --release -p noiria-core --example bench

use std::sync::Arc;
use std::time::Instant;

use noiria_core::compose::{compose_frame, ComposeBufs};
use noiria_core::pixmap::Pixmap;
use noiria_core::scene::{LayerKind, Page, Scene};
use noiria_core::transition::{TransMethod, Transition};

const W: u32 = 1280;
const H: u32 = 720;

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

fn bench(name: &str, mut f: impl FnMut()) {
    // ウォームアップ
    for _ in 0..3 {
        f();
    }
    let iters = 60;
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    let per = t0.elapsed().as_secs_f64() * 1000.0 / iters as f64;
    println!("{name:<32} {per:7.3} ms/frame");
}

fn main() {
    let mut seed = 7u64;
    let opaque = noise(W, H, &mut seed, false);
    let translucent = noise(W, H, &mut seed, true);
    let small = noise(500, 900, &mut seed, true);
    let mut dst = Pixmap::new(W, H);
    let a = noise(W, H, &mut seed, false);
    let b = noise(W, H, &mut seed, false);
    let rule: Vec<u8> = (0..W * H).map(|i| (i % 256) as u8).collect();

    bench("blit_over 不透明フル画面", || {
        dst.blit_over(&opaque, 0, 0, 255);
    });
    bench("blit_over 半透明フル画面", || {
        dst.blit_over(&translucent, 0, 0, 255);
    });
    bench("blit_over 不透明 opacity=128", || {
        dst.blit_over(&opaque, 0, 0, 128);
    });
    bench("blit_over 立ち絵 500x900", || {
        dst.blit_over(&small, 390, 60, 255);
    });
    bench("lerp_from (クロスフェード)", || {
        dst.lerp_from(&a, &b, 0.5);
    });
    bench("universal_from (ルール)", || {
        dst.universal_from(&a, &b, &rule, 0.5, 64.0);
    });
    let mut out32 = vec![0u32; (W * H) as usize];
    bench("to_bgrx_u32", || {
        dst.to_bgrx_u32(&mut out32);
    });

    // 実践的シーン: 背景 + 立ち絵2 + メッセージボックス
    let mut scene = Scene::new(W, H);
    let bg = scene.alloc_layer(LayerKind::Image);
    let ch1 = scene.alloc_layer(LayerKind::Image);
    let ch2 = scene.alloc_layer(LayerKind::Image);
    let msg = scene.alloc_layer(LayerKind::Image);
    for (id, pm, x, z) in [
        (bg, opaque.clone(), 0, 0),
        (ch1, small.clone(), 200, 10),
        (ch2, small.clone(), 600, 11),
    ] {
        let l = scene.layer_mut(Page::Fore, id).unwrap();
        l.pixmap = Some(Arc::new(pm));
        l.x = x;
        l.z = z;
        l.visible = true;
    }
    {
        let mut box_pm = Pixmap::new(1200, 176);
        box_pm.fill_rounded_rect(0, 0, 1200, 176, 14, [10, 12, 30, 195]);
        let l = scene.layer_mut(Page::Fore, msg).unwrap();
        l.pixmap = Some(Arc::new(box_pm));
        l.x = 40;
        l.y = 520;
        l.z = 100;
        l.visible = true;
    }
    scene.backlay(None);

    let mut bufs = ComposeBufs::new(W, H);
    let mut frame = Pixmap::new(W, H);
    let now = Instant::now();
    bench("compose 4レイヤ (通常)", || {
        compose_frame(&scene, now, &mut bufs, &mut frame);
    });

    scene.transition = Some(Transition {
        method: TransMethod::Crossfade,
        start: now,
        duration: std::time::Duration::from_secs(100),
        skip: false,
    });
    bench("compose クロスフェード中", || {
        compose_frame(&scene, Instant::now(), &mut bufs, &mut frame);
    });
    scene.transition = Some(Transition {
        method: TransMethod::Universal {
            rule: Arc::new(rule.clone()),
            vague: 64.0,
        },
        start: now,
        duration: std::time::Duration::from_secs(100),
        skip: false,
    });
    bench("compose ルールトランジション中", || {
        compose_frame(&scene, Instant::now(), &mut bufs, &mut frame);
    });
}
