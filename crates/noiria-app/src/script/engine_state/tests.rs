//! `layer_blit_image`（W4）: V8/tsint を介さず `EngineState` を直接叩く単体テスト
//! （`script::mod::js_tests::move_tween_reaches_to_value_in_v8` と同じ
//! 「実ファイル + 実 GameDir」方式だが、こちらはスクリプトエンジンを経由しない
//! ぶん軽量）。3 状態ボタン画像（横に 3 分割、各 4x4px の色違い）を模した
//! PNG を一時ゲームディレクトリへ書き出し、clip= で中央（緑）だけを
//! 切り出してレイヤへブリットできることを確認する。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use crossbeam_channel::unbounded;
use noiria_core::assets::{save_png, GameDir};
use noiria_core::codec::video::VideoSource;
use noiria_core::scene::{Page, Scene};

use super::video::{
    next_video_pixmap, playback_loop, push_recycle, PlaybackControl, VideoCommitHook,
    VideoEventKind, VideoEvents,
};
use super::*;

fn make_engine_state(dir: &std::path::Path) -> EngineState {
    let (game_dir, config) = GameDir::open(dir).expect("GameDir::open");
    let (user_tx, _user_rx) = unbounded();
    let (audio_tx, audio_rx) = unbounded();
    drop(audio_rx);
    EngineState::new(
        Arc::new(Mutex::new(Scene::new(64, 64))),
        WakeSink::Channel(user_tx),
        Arc::new(Notify::new()),
        audio_tx,
        Arc::new(AudioShared::default()),
        game_dir,
        config,
        Instant::now(),
        Clock::Real,
        InputSync::new(),
        TickSync::new(),
        PendingWaits::new(),
        None,
    )
}

#[test]
fn layer_blit_image_clips_and_composites_onto_existing_layer() {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("noiria-blit-image-test-{ns}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("game.json"),
        r#"{"title":"t","id":"noiria-blit-image-test","main":"main.ts"}"#,
    )
    .unwrap();

    // 12x4 の画像を 4x4 の 3 マス (赤/緑/青) に分割した「3 状態ボタン」スプライト。
    let mut sprite = Pixmap::new(12, 4);
    sprite.fill([0, 0, 0, 0]);
    let red = Pixmap::from_rgba_straight(4, 4, &[255, 0, 0, 255].repeat(4 * 4));
    let green = Pixmap::from_rgba_straight(4, 4, &[0, 255, 0, 255].repeat(4 * 4));
    let blue = Pixmap::from_rgba_straight(4, 4, &[0, 0, 255, 255].repeat(4 * 4));
    sprite.blit_over(&red, 0, 0, 255);
    sprite.blit_over(&green, 4, 0, 255);
    sprite.blit_over(&blue, 8, 0, 255);
    save_png(&sprite, &dir.join("button.png")).unwrap();

    let es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    // 既存レイヤへの上書き（既存内容を消さない）を確認するため、先に透明キャンバスを確保。
    es.layer_fill(id, "fore", 4, 4, 0, 0, 0, 0).unwrap();
    // 中央 (over/緑) だけを clip して (0,0) へブリット。
    es.layer_blit_image(
        id,
        "fore",
        "button.png",
        0,
        0,
        Some(BlitClip {
            x: 4,
            y: 0,
            w: 4,
            h: 4,
        }),
    )
    .unwrap();

    let scene = es.scene.lock().unwrap();
    let layer = scene.layer(Page::Fore, id).expect("レイヤが存在するはず");
    let pixmap = layer.pixmap.as_ref().expect("blit 後は pixmap があるはず");
    assert_eq!((pixmap.width, pixmap.height), (4, 4));
    for px in pixmap.data.as_chunks::<4>().0 {
        assert_eq!(px, &[0, 255, 0, 255], "clip した緑マスのみが写るはず");
    }
    drop(scene);

    std::fs::remove_dir_all(&dir).ok();
}

fn temp_game_dir(name: &str) -> std::path::PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("noiria-{name}-{ns}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("game.json"),
        format!(r#"{{"title":"t","id":"noiria-{name}","main":"main.ts"}}"#),
    )
    .unwrap();
    dir
}

#[test]
fn audio_play_accepts_finite_nonnegative_start_and_rejects_invalid_values() {
    let dir = temp_game_dir("audio-start-position-test");
    let es = make_engine_state(&dir);
    for start in [0.0, 2345.0, f64::MAX] {
        assert!(
            es.audio_play("bgm", "clip.wav", true, 1.0, 0, start, false)
                .is_ok(),
            "有限の非負値は受理する: {start}"
        );
    }
    for start in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            es.audio_play("bgm", "clip.wav", true, 1.0, 0, start, false)
                .is_err(),
            "異常値はエラーにする: {start}"
        );
    }
    assert_eq!(es.audio_position("bgm").unwrap(), u64::MAX);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn audio_play_exposes_requested_start_before_audio_thread_acknowledges_it() {
    let dir = temp_game_dir("audio-start-position-eager-test");
    let es = make_engine_state(&dir);
    es.audio_shared
        .mark_play_requested(noiria_core::audio::CH_BGM, 111);

    es.audio_play("bgm", "clip.wav", true, 1.0, 0, 2345.0, true)
        .unwrap();

    assert_eq!(es.audio_position("bgm").unwrap(), 2345);
    std::fs::remove_dir_all(&dir).ok();
}

/// [stopquake]（W5）: quake_stop が scene.quake を即座にクリアすることを確認する。
#[test]
fn quake_stop_clears_active_quake() {
    let dir = temp_game_dir("quake-stop-test");
    let es = make_engine_state(&dir);
    es.quake(5, 5, 5000);
    assert!(
        es.scene.lock().unwrap().quake.is_some(),
        "quake 直後は Some のはず"
    );
    es.quake_stop();
    assert!(
        es.scene.lock().unwrap().quake.is_none(),
        "quake_stop 後は None のはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// [move path=]（W5）: layer_move_path が第一区間を tween、残りを tween_queue へ
/// 積むことを確認する（区間をまたぐ継ぎ目のロジック自体は scene.rs の
/// `settle_finished_tweens_chains_to_next_waypoint_seamlessly` 等で検証済み。
/// ここでは ops 経由の配線・省略値の引き継ぎだけを確認する）。
#[test]
fn layer_move_path_starts_first_leg_and_queues_the_rest() {
    let dir = temp_game_dir("move-path-test");
    let es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    es.layer_move_path(
        id,
        "fore",
        vec![
            MoveArgs {
                to_x: Some(50),
                to_y: None,
                to_opacity: None,
                time_ms: 100,
                accel: 0.0,
            },
            MoveArgs {
                to_x: Some(150),
                to_y: Some(80),
                to_opacity: Some(128),
                time_ms: 200,
                accel: 0.5,
            },
        ],
        false,
    )
    .unwrap();
    let scene = es.scene.lock().unwrap();
    let layer = scene.layer(Page::Fore, id).unwrap();
    let t = layer
        .tween
        .as_ref()
        .expect("第一区間の tween が開始しているはず");
    assert_eq!(
        (t.to_x, t.to_y, t.to_opacity),
        (50, 0, 255),
        "第一区間: y/opacity は現在値を引き継ぐ"
    );
    assert!(t.spline.is_none());
    assert_eq!(
        layer.tween_queue.len(),
        1,
        "残り区間は tween_queue へ積まれるはず"
    );
    let wp = &layer.tween_queue[0];
    assert_eq!((wp.x, wp.y, wp.opacity), (150, 80, 128));
    assert!(wp.spline.is_none());
    drop(scene);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn layer_move_path_rejects_short_spline_and_attaches_segments_on_back_page() {
    let dir = temp_game_dir("move-spline-test");
    let es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    let short = vec![MoveArgs {
        to_x: Some(100),
        to_y: Some(100),
        to_opacity: None,
        time_ms: 100,
        accel: 0.0,
    }];
    assert!(es.layer_move_path(id, "back", short, true).is_err());
    assert!(es
        .scene
        .lock()
        .unwrap()
        .layer(Page::Back, id)
        .unwrap()
        .tween
        .is_none());

    es.layer_move_path(
        id,
        "back",
        vec![
            MoveArgs {
                to_x: Some(100),
                to_y: Some(100),
                to_opacity: Some(100),
                time_ms: 100,
                accel: 0.0,
            },
            MoveArgs {
                to_x: Some(0),
                to_y: Some(0),
                to_opacity: Some(200),
                time_ms: 100,
                accel: 0.0,
            },
        ],
        true,
    )
    .unwrap();

    let scene = es.scene.lock().unwrap();
    assert!(scene.layer(Page::Fore, id).unwrap().tween.is_none());
    let layer = scene.layer(Page::Back, id).unwrap();
    let first = layer.tween.as_ref().unwrap();
    assert!(first.spline.is_some());
    assert_eq!(first.current_x(first.start + Duration::from_millis(50)), 69);
    assert_eq!(
        first.current_opacity(first.start + Duration::from_millis(50)),
        178
    );
    assert_eq!(layer.tween_queue.len(), 1);
    assert!(layer.tween_queue[0].spline.is_some());
    drop(scene);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn layer_moving_includes_pending_queue_after_current_leg_finishes() {
    let dir = temp_game_dir("move-queue-moving-test");
    let es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    es.layer_move_path(
        id,
        "fore",
        vec![
            MoveArgs {
                to_x: Some(100),
                to_y: None,
                to_opacity: None,
                time_ms: 0,
                accel: 0.0,
            },
            MoveArgs {
                to_x: Some(200),
                to_y: None,
                to_opacity: None,
                time_ms: 10_000,
                accel: 0.0,
            },
        ],
        true,
    )
    .unwrap();
    {
        let scene = es.scene.lock().unwrap();
        let layer = scene.layer(Page::Fore, id).unwrap();
        assert!(layer.tween.as_ref().unwrap().is_finished(es.clock.now()));
        assert_eq!(layer.tween_queue.len(), 1);
    }
    assert!(es.layer_moving());
    es.layer_move_skip();
    assert!(!es.layer_moving());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn single_move_interrupts_spline_path_and_discards_pending_waypoints() {
    let dir = temp_game_dir("move-spline-interrupt-test");
    let es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    es.layer_move_path(
        id,
        "fore",
        vec![
            MoveArgs {
                to_x: Some(100),
                to_y: Some(100),
                to_opacity: None,
                time_ms: 10_000,
                accel: 0.0,
            },
            MoveArgs {
                to_x: Some(200),
                to_y: Some(0),
                to_opacity: None,
                time_ms: 10_000,
                accel: 0.0,
            },
        ],
        true,
    )
    .unwrap();
    es.layer_move(
        id,
        "fore",
        MoveArgs {
            to_x: Some(25),
            to_y: Some(50),
            to_opacity: Some(128),
            time_ms: 100,
            accel: 0.0,
        },
    )
    .unwrap();

    let scene = es.scene.lock().unwrap();
    let layer = scene.layer(Page::Fore, id).unwrap();
    assert!(layer.tween.as_ref().unwrap().spline.is_none());
    assert!(layer.tween_queue.is_empty());
    let end = layer.tween.as_ref().unwrap().start + Duration::from_millis(100);
    drop(scene);

    let mut scene = es.scene.lock().unwrap();
    scene.settle_finished_tweens(end);
    let layer = scene.layer(Page::Fore, id).unwrap();
    assert!(layer.tween.is_none());
    assert_eq!((layer.x, layer.y, layer.opacity), (25, 50, 128));
    drop(scene);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn distance_allocated_path_samples_effective_position_when_interrupting() {
    let dir = temp_game_dir("move-path-distance-interrupt-test");
    let mut es = make_engine_state(&dir);
    let base = Instant::now();
    es.clock = Clock::new_virtual(base);
    let id = es.layer_new("image").unwrap();
    es.layer_move(
        id,
        "fore",
        MoveArgs {
            to_x: Some(100),
            to_y: None,
            to_opacity: None,
            time_ms: 1000,
            accel: 0.0,
        },
    )
    .unwrap();
    es.clock.advance(Duration::from_millis(500));

    es.layer_move_path_by_distance(
        id,
        "fore",
        vec![
            MoveArgs {
                to_x: Some(100),
                to_y: None,
                to_opacity: None,
                time_ms: 7,
                accel: 0.0,
            },
            MoveArgs {
                to_x: Some(300),
                to_y: None,
                to_opacity: None,
                time_ms: 9,
                accel: 0.0,
            },
        ],
        true,
        2500.0,
    )
    .unwrap();

    let scene = es.scene.lock().unwrap();
    let layer = scene.layer(Page::Fore, id).unwrap();
    let first = layer.tween.as_ref().unwrap();
    assert_eq!(first.from_x, 50);
    assert_eq!(first.duration, Duration::from_millis(500));
    assert_eq!(layer.tween_queue[0].duration, Duration::from_millis(2000));
    drop(scene);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn distance_allocated_path_handles_zero_distance_and_inherited_coordinates() {
    let dir = temp_game_dir("move-path-distance-edge-test");
    let es = make_engine_state(&dir);
    let stationary_id = es.layer_new("image").unwrap();
    es.layer_move_path_by_distance(
        stationary_id,
        "fore",
        vec![
            MoveArgs {
                to_opacity: Some(128),
                ..MoveArgs::default()
            },
            MoveArgs {
                to_opacity: Some(64),
                ..MoveArgs::default()
            },
        ],
        false,
        300.0,
    )
    .unwrap();
    let inherited_id = es.layer_new("image").unwrap();
    es.layer_move_path_by_distance(
        inherited_id,
        "fore",
        vec![
            MoveArgs {
                to_x: Some(100),
                ..MoveArgs::default()
            },
            MoveArgs {
                to_y: Some(100),
                ..MoveArgs::default()
            },
        ],
        false,
        200.0,
    )
    .unwrap();

    let scene = es.scene.lock().unwrap();
    let stationary = scene.layer(Page::Fore, stationary_id).unwrap();
    assert_eq!(
        stationary.tween.as_ref().unwrap().duration,
        Duration::from_millis(150)
    );
    assert_eq!(
        stationary.tween_queue[0].duration,
        Duration::from_millis(150)
    );
    let inherited = scene.layer(Page::Fore, inherited_id).unwrap();
    assert_eq!(
        inherited.tween.as_ref().unwrap().duration,
        Duration::from_millis(100)
    );
    assert_eq!(
        inherited.tween_queue[0].duration,
        Duration::from_millis(100)
    );
    drop(scene);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn layer_move_cancel_discards_active_path_without_applying_its_endpoint() {
    let dir = temp_game_dir("move-path-cancel-test");
    let es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    es.layer_move_path(
        id,
        "fore",
        vec![
            MoveArgs {
                to_x: Some(100),
                to_y: Some(50),
                to_opacity: None,
                time_ms: 1000,
                accel: 0.0,
            },
            MoveArgs {
                to_x: Some(200),
                to_y: Some(0),
                to_opacity: None,
                time_ms: 1000,
                accel: 0.0,
            },
        ],
        true,
    )
    .unwrap();

    es.layer_move_cancel(id, "fore").unwrap();

    let scene = es.scene.lock().unwrap();
    let layer = scene.layer(Page::Fore, id).unwrap();
    assert!(layer.tween.is_none());
    assert!(layer.tween_queue.is_empty());
    assert_eq!((layer.x, layer.y), (0, 0));
    drop(scene);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn text_reflow_preserves_active_reveal_and_refreshes_glyph_cache() {
    let dir = temp_game_dir("text-reflow-reveal-test");
    let mut es = make_engine_state(&dir);
    let base = Instant::now();
    es.clock = Clock::new_virtual(base);
    let id = es.layer_new("text").unwrap();
    let mut style = TextStyle {
        box_w: 300,
        ..TextStyle::default()
    };
    es.text_style(id, style.clone());
    es.text_print(
        id,
        vec![TextSegment {
            text: "AB".to_string(),
            ..TextSegment::default()
        }],
        false,
        0.0,
    )
    .unwrap();
    es.text_print(
        id,
        vec![TextSegment {
            text: "CDE".to_string(),
            ..TextSegment::default()
        }],
        true,
        1.0,
    )
    .unwrap();

    let (before_reveal, before_raster) = {
        let scene = es.scene.lock().unwrap();
        let text = scene.layer(Page::Fore, id).unwrap().text.as_ref().unwrap();
        (text.reveal.clone(), text.raster.clone())
    };
    style.align = noiria_core::text::TextAlign::Right;
    es.text_style(id, style);
    es.text_reflow(id).unwrap();

    let scene = es.scene.lock().unwrap();
    let text = scene.layer(Page::Fore, id).unwrap().text.as_ref().unwrap();
    assert_eq!(text.reveal.from_glyph, before_reveal.from_glyph);
    assert_eq!(text.reveal.start, before_reveal.start);
    assert_eq!(text.reveal.cps, before_reveal.cps);
    assert!(!Arc::ptr_eq(&text.raster, &before_raster));
    assert_eq!(
        es.texts.glyph_counts.get(&id).copied(),
        Some(text.raster.glyphs.len())
    );
    drop(scene);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn layer_blit_image_creates_canvas_when_layer_has_no_pixmap_yet() {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("noiria-blit-image-test2-{ns}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("game.json"),
        r#"{"title":"t","id":"noiria-blit-image-test2","main":"main.ts"}"#,
    )
    .unwrap();
    let mut img = Pixmap::new(3, 3);
    img.fill([10, 20, 30, 255]);
    save_png(&img, &dir.join("solid.png")).unwrap();

    let es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    // clip 省略 = 画像全体。pixmap 未確保のレイヤでも自動でキャンバスができる。
    es.layer_blit_image(id, "fore", "solid.png", 0, 0, None)
        .unwrap();

    let scene = es.scene.lock().unwrap();
    let layer = scene.layer(Page::Fore, id).unwrap();
    let pixmap = layer.pixmap.as_ref().unwrap();
    assert_eq!((pixmap.width, pixmap.height), (3, 3));
    drop(scene);

    std::fs::remove_dir_all(&dir).ok();
}

// ---- 動画 ops (W6: openvideo/preparevideo/playvideo/pausevideo/resumevideo/rewindvideo) ----
// W6 では JPEG エンコードした極小フレーム列を自前で AVI (MJPEG) コンテナへ詰めて実行時に
// 動的生成していた（`image` クレートを dev-dependency として使用）。W7 でリポジトリに
// 固定フィクスチャとして同梱する方式へ変更し、`noiria-app` の `image` dev-dependency を
// 削除した（`docs/implementation-notes.md`「2026-07-11 W7」参照）。フィクスチャ自体は
// 生成当時と全く同じロジック（16x16 JPEG フレーム + `write_mjpeg_avi`）で 1 度だけ作成
// したバイト列（`tests/fixtures/clip_2frames.avi`: shade=40,220 の 2 フレーム。
// `tests/fixtures/clip_5frames.avi`: shade=10,20,30,40,50 の 5 フレーム）。

static FIXTURE_CLIP_2FRAMES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/clip_2frames.avi"
));
static FIXTURE_CLIP_5FRAMES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/clip_5frames.avi"
));
static FIXTURE_VP9_16X16: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../noiria-core/tests/fixtures/vp9_p2_10bit_yuv420_16x16_2f.ivf"
));

struct ChangingFrameSource {
    cursor: usize,
    width: u32,
    height: u32,
}

impl ChangingFrameSource {
    fn new() -> Self {
        Self {
            cursor: 0,
            width: 2,
            height: 1,
        }
    }
}

impl VideoSource for ChangingFrameSource {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn fps(&self) -> f32 {
        240.0
    }

    fn next_frame(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        let frame = match self.cursor {
            0 => {
                self.width = 2;
                self.height = 1;
                [255, 0, 0, 255].repeat(2)
            }
            1 => {
                self.width = 3;
                self.height = 2;
                [0, 255, 0, 255].repeat(6)
            }
            _ => return Ok(None),
        };
        self.cursor += 1;
        Ok(Some(frame))
    }

    fn rewind(&mut self) -> anyhow::Result<()> {
        self.cursor = 0;
        self.width = 2;
        self.height = 1;
        Ok(())
    }
}

struct PanickingVideoSource;

impl VideoSource for PanickingVideoSource {
    fn width(&self) -> u32 {
        1
    }

    fn height(&self) -> u32 {
        1
    }

    fn fps(&self) -> f32 {
        30.0
    }

    fn next_frame(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        panic!("intentional video worker failure")
    }

    fn rewind(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

struct IndexedVideoSource {
    cursor: usize,
    frame_count: usize,
    fps: f32,
    base: u8,
    delay: Duration,
    decoded: Arc<Mutex<Vec<usize>>>,
}

impl IndexedVideoSource {
    fn new(
        frame_count: usize,
        fps: f32,
        base: u8,
        delay: Duration,
        decoded: Arc<Mutex<Vec<usize>>>,
    ) -> Self {
        Self {
            cursor: 0,
            frame_count,
            fps,
            base,
            delay,
            decoded,
        }
    }
}

impl VideoSource for IndexedVideoSource {
    fn width(&self) -> u32 {
        1
    }

    fn height(&self) -> u32 {
        1
    }

    fn fps(&self) -> f32 {
        self.fps
    }

    fn next_frame(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        if self.cursor >= self.frame_count {
            return Ok(None);
        }
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        let index = self.cursor;
        self.cursor += 1;
        self.decoded.lock().unwrap().push(index);
        Ok(Some(vec![self.base.saturating_add(index as u8), 0, 0, 255]))
    }

    fn rewind(&mut self) -> anyhow::Result<()> {
        self.cursor = 0;
        Ok(())
    }

    fn seek_frame(&mut self, target: usize) -> anyhow::Result<()> {
        anyhow::ensure!(
            target <= self.frame_count,
            "test frame target out of range: {target}"
        );
        self.cursor = target;
        Ok(())
    }
}

struct BlockOnceVideoSource {
    cursor: usize,
    frame_count: usize,
    fps: f32,
    base: u8,
    blocked: bool,
    started: std::sync::mpsc::Sender<usize>,
    release: std::sync::mpsc::Receiver<()>,
    dropped: std::sync::mpsc::Sender<()>,
}

impl VideoSource for BlockOnceVideoSource {
    fn width(&self) -> u32 {
        1
    }

    fn height(&self) -> u32 {
        1
    }

    fn fps(&self) -> f32 {
        self.fps
    }

    fn next_frame(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        if !self.blocked {
            self.blocked = true;
            self.started
                .send(self.cursor)
                .map_err(|e| anyhow::anyhow!("decode start signal failed: {e}"))?;
            self.release
                .recv_timeout(Duration::from_secs(2))
                .map_err(|e| anyhow::anyhow!("decode release signal failed: {e}"))?;
        }
        if self.cursor >= self.frame_count {
            return Ok(None);
        }
        let index = self.cursor;
        self.cursor += 1;
        Ok(Some(vec![self.base.saturating_add(index as u8), 0, 0, 255]))
    }

    fn rewind(&mut self) -> anyhow::Result<()> {
        self.cursor = 0;
        Ok(())
    }

    fn seek_frame(&mut self, target: usize) -> anyhow::Result<()> {
        anyhow::ensure!(
            target <= self.frame_count,
            "test frame target out of range: {target}"
        );
        self.cursor = target;
        Ok(())
    }
}

impl Drop for BlockOnceVideoSource {
    fn drop(&mut self) {
        let _ = self.dropped.send(());
    }
}

fn block_once_video_source(
    cursor: usize,
    frame_count: usize,
    fps: f32,
    base: u8,
) -> (
    BlockOnceVideoSource,
    std::sync::mpsc::Receiver<usize>,
    std::sync::mpsc::Sender<()>,
    std::sync::mpsc::Receiver<()>,
) {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
    (
        BlockOnceVideoSource {
            cursor,
            frame_count,
            fps,
            base,
            blocked: false,
            started: started_tx,
            release: release_rx,
            dropped: dropped_tx,
        },
        started_rx,
        release_tx,
        dropped_rx,
    )
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("condition was not met within {timeout:?}");
}

/// 固定フィクスチャ（2 フレーム、shade 40→220）を一時ゲームディレクトリへ書き出す。
fn write_test_avi_2frames(dir: &std::path::Path, name: &str) {
    std::fs::write(dir.join(name), FIXTURE_CLIP_2FRAMES).unwrap();
}

/// 固定フィクスチャ（5 フレーム、shade 10,20,30,40,50）を一時ゲームディレクトリへ書き出す。
fn write_test_avi_5frames(dir: &std::path::Path, name: &str) {
    std::fs::write(dir.join(name), FIXTURE_CLIP_5FRAMES).unwrap();
}

#[test]
fn video_open_does_not_touch_layer_pixmap_or_start_playback() {
    let dir = temp_game_dir("video-open-test");
    write_test_avi_2frames(&dir, "clip.avi");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    es.video_open(id, "clip.avi", false).unwrap();
    {
        let scene = es.scene.lock().unwrap();
        let layer = scene.layer(Page::Fore, id).unwrap();
        assert!(
            layer.pixmap.is_none(),
            "video_open だけではレイヤへ書き込まないはず"
        );
    }
    assert!(
        !es.video_is_playing(),
        "video_open だけでは再生は始まらない"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_prepare_shows_first_frame_and_does_not_start_playback() {
    let dir = temp_game_dir("video-prepare-test");
    write_test_avi_2frames(&dir, "clip.avi");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    es.video_prepare(id, "clip.avi", false).unwrap();
    {
        let scene = es.scene.lock().unwrap();
        let layer = scene.layer(Page::Fore, id).unwrap();
        let pixmap = layer
            .pixmap
            .as_ref()
            .expect("先頭フレームが表示されているはず");
        let center = ((pixmap.height / 2 * pixmap.width + pixmap.width / 2) * 4) as usize;
        assert!(
            (pixmap.data[center] as i32 - 40).abs() <= 12,
            "1 枚目 (shade=40) が表示されているはず: R={}",
            pixmap.data[center]
        );
    }
    assert!(!es.video_is_playing(), "prepare だけでは再生は始まらない");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_prepare_uses_decoded_frame_dimensions() {
    let dir = temp_game_dir("video-prepare-frame-size-test");
    let mut ivf = FIXTURE_VP9_16X16.to_vec();
    ivf[12..14].copy_from_slice(&1u16.to_le_bytes());
    ivf[14..16].copy_from_slice(&1u16.to_le_bytes());
    std::fs::write(dir.join("clip.ivf"), ivf).unwrap();
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();

    es.video_prepare(id, "clip.ivf", false).unwrap();

    let scene = es.scene.lock().unwrap();
    let pixmap = scene
        .layer(Page::Fore, id)
        .unwrap()
        .pixmap
        .as_ref()
        .unwrap();
    assert_eq!((pixmap.width, pixmap.height), (16, 16));
    assert_eq!(pixmap.data.len(), 16 * 16 * 4);
    drop(scene);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_prepared_seek_prepare_latch_period_and_segment_loop_are_exact() {
    let dir = temp_game_dir("video-b4-prepared-test");
    write_test_avi_2frames(&dir, "clip.avi");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();

    es.video_open(id, "clip.avi", false).unwrap();
    es.video_seek_frame(1).unwrap();
    es.video_set_period_event(1);
    es.video_set_segment_loop(0, 2).unwrap();
    es.video_prepare_opened(id).unwrap();

    {
        let scene = es.scene.lock().unwrap();
        let pixmap = scene
            .layer(Page::Fore, id)
            .unwrap()
            .pixmap
            .as_ref()
            .unwrap();
        let center = ((pixmap.height / 2 * pixmap.width + pixmap.width / 2) * 4) as usize;
        assert!(
            (pixmap.data[center] as i32 - 220).abs() <= 12,
            "seek target frame was not used for prepare preview: {}",
            pixmap.data[center]
        );
    }
    assert!(es.video_take_event("prepare"));
    assert!(
        !es.video_take_event("prepare"),
        "prepare latch must be one-shot"
    );

    es.video_play_prepared(id).unwrap();
    let current_events = es.videos.current.as_ref().unwrap().events.clone();
    wait_until(Duration::from_secs(2), || es.video_take_event("period"));
    wait_until(Duration::from_secs(2), || {
        current_events.lock().unwrap().seg_loop_count >= 2
    });
    assert!(
        !es.video_take_event("period"),
        "period event must remain cleared after crossing the target again"
    );

    es.video_cancel_segment_loop();
    wait_until(Duration::from_secs(2), || !es.video_is_playing());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_position_seek_updates_prepared_preview() {
    let dir = temp_game_dir("video-b4-position-test");
    write_test_avi_2frames(&dir, "clip.avi");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    es.video_open(id, "clip.avi", false).unwrap();
    let fps = es.videos.prepared.as_ref().unwrap().source.fps() as f64;

    assert_eq!(video_optional_frame(-1.0).unwrap(), None);
    assert_eq!(
        video_optional_frame(u32::MAX as f64).unwrap(),
        Some(u32::MAX as usize)
    );
    assert!(video_optional_frame(-2.0).is_err());
    assert!(video_optional_frame(1.5).is_err());
    assert!(video_optional_frame(u32::MAX as f64 + 1.0).is_err());
    assert!(es.video_seek_position(-1.0).is_err());
    assert!(es.video_seek_position(f64::NAN).is_err());
    assert!(es.video_seek_position(f64::INFINITY).is_err());
    es.video_seek_position(1500.0 / fps).unwrap();
    es.video_prepare_opened(id).unwrap();

    let scene = es.scene.lock().unwrap();
    let pixmap = scene
        .layer(Page::Fore, id)
        .unwrap()
        .pixmap
        .as_ref()
        .unwrap();
    let center = ((pixmap.height / 2 * pixmap.width + pixmap.width / 2) * 4) as usize;
    assert!(
        (pixmap.data[center] as i32 - 220).abs() <= 12,
        "position seek preview did not select frame 1: {}",
        pixmap.data[center]
    );
    drop(scene);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_commit_gate_drops_decoded_frame_when_seek_returns() {
    let dir = temp_game_dir("video-b4-seek-race-test");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    let events = Arc::new(Mutex::new(VideoEvents::default()));
    let (source, started, release, dropped) = block_once_video_source(0, 3, 240.0, 10);

    es.spawn_video_playback(
        id,
        Box::new(source),
        false,
        0,
        Arc::new(Mutex::new(PlaybackControl::default())),
        events.clone(),
    );
    assert_eq!(started.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
    es.video_seek_frame(2).unwrap();
    release.send(()).unwrap();
    dropped.recv_timeout(Duration::from_secs(2)).unwrap();

    let events = events.lock().unwrap();
    assert_eq!(events.displayed_count, 1);
    assert_eq!(events.last_displayed_frame, Some(2));
    drop(events);
    let scene = es.scene.lock().unwrap();
    assert_eq!(
        scene
            .layer(Page::Fore, id)
            .unwrap()
            .pixmap
            .as_ref()
            .unwrap()
            .data,
        [12, 0, 0, 255]
    );
    drop(scene);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_commit_gate_observes_period_set_during_decode() {
    let dir = temp_game_dir("video-b4-period-race-test");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    let events = Arc::new(Mutex::new(VideoEvents::default()));
    let (source, started, decode_release, dropped) = block_once_video_source(1, 2, 240.0, 20);
    let (commit_reached_tx, commit_reached_rx) = std::sync::mpsc::channel();
    let (commit_release_tx, commit_release_rx) = std::sync::mpsc::channel();
    let control = Arc::new(Mutex::new(PlaybackControl::default()));
    control.lock().unwrap().before_commit_hook = Some(VideoCommitHook {
        reached: commit_reached_tx,
        release: commit_release_rx,
    });

    es.spawn_video_playback(id, Box::new(source), false, 1, control, events.clone());
    assert_eq!(started.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
    decode_release.send(()).unwrap();
    commit_reached_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    es.video_set_period_event(1);
    commit_release_tx.send(()).unwrap();
    wait_until(Duration::from_secs(2), || es.video_take_event("period"));
    dropped.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(events.lock().unwrap().last_displayed_frame, Some(1));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_commit_gate_rechecks_segment_set_during_decode() {
    let dir = temp_game_dir("video-b4-segment-race-test");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    let events = Arc::new(Mutex::new(VideoEvents::default()));
    let (source, started, release, dropped) = block_once_video_source(3, 4, 1.0, 30);

    es.spawn_video_playback(
        id,
        Box::new(source),
        false,
        3,
        Arc::new(Mutex::new(PlaybackControl::default())),
        events.clone(),
    );
    assert_eq!(started.recv_timeout(Duration::from_secs(2)).unwrap(), 3);
    es.video_set_segment_loop(1, 3).unwrap();
    release.send(()).unwrap();
    wait_until(Duration::from_secs(2), || {
        events.lock().unwrap().displayed_count >= 1
    });
    {
        let events = events.lock().unwrap();
        assert_eq!(events.displayed_count, 1);
        assert_eq!(events.last_displayed_frame, Some(1));
        assert_eq!(events.seg_loop_count, 1);
    }
    let scene = es.scene.lock().unwrap();
    assert_eq!(
        scene
            .layer(Page::Fore, id)
            .unwrap()
            .pixmap
            .as_ref()
            .unwrap()
            .data,
        [31, 0, 0, 255]
    );
    drop(scene);
    es.video_stop();
    dropped.recv_timeout(Duration::from_secs(2)).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_eof_gate_honors_seek_set_during_eof_read() {
    let dir = temp_game_dir("video-b4-eof-race-test");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    let events = Arc::new(Mutex::new(VideoEvents::default()));
    let (source, started, release, dropped) = block_once_video_source(2, 2, 240.0, 40);

    es.spawn_video_playback(
        id,
        Box::new(source),
        false,
        2,
        Arc::new(Mutex::new(PlaybackControl::default())),
        events.clone(),
    );
    assert_eq!(started.recv_timeout(Duration::from_secs(2)).unwrap(), 2);
    es.video_seek_frame(1).unwrap();
    release.send(()).unwrap();
    dropped.recv_timeout(Duration::from_secs(2)).unwrap();

    let events = events.lock().unwrap();
    assert_eq!(events.displayed_count, 1);
    assert_eq!(events.last_displayed_frame, Some(1));
    drop(events);
    let scene = es.scene.lock().unwrap();
    assert_eq!(
        scene
            .layer(Page::Fore, id)
            .unwrap()
            .pixmap
            .as_ref()
            .unwrap()
            .data,
        [41, 0, 0, 255]
    );
    drop(scene);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_stop_is_inactive_before_blocked_decode_returns() {
    let dir = temp_game_dir("video-b4-stop-race-test");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    let events = Arc::new(Mutex::new(VideoEvents::default()));
    let (source, started, release, dropped) = block_once_video_source(0, 1, 240.0, 50);

    es.spawn_video_playback(
        id,
        Box::new(source),
        false,
        0,
        Arc::new(Mutex::new(PlaybackControl::default())),
        events.clone(),
    );
    assert_eq!(started.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
    es.video_stop();
    assert!(!es.video_is_playing());
    release.send(()).unwrap();
    dropped.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(events.lock().unwrap().displayed_count, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_segment_loop_is_end_exclusive_and_cancel_resumes_to_eof() {
    let dir = temp_game_dir("video-b4-segment-test");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    let decoded = Arc::new(Mutex::new(Vec::new()));
    let control = Arc::new(Mutex::new(PlaybackControl::default()));
    control.lock().unwrap().segment_loop = Some((1, 4));
    let events = Arc::new(Mutex::new(VideoEvents::default()));

    es.spawn_video_playback(
        id,
        Box::new(IndexedVideoSource::new(
            5,
            240.0,
            10,
            Duration::ZERO,
            decoded.clone(),
        )),
        false,
        0,
        control,
        events.clone(),
    );
    wait_until(Duration::from_secs(2), || {
        events.lock().unwrap().seg_loop_count > 0 && decoded.lock().unwrap().len() >= 8
    });
    es.video_cancel_segment_loop();
    wait_until(Duration::from_secs(2), || !es.video_is_playing());

    let sequence = decoded.lock().unwrap().clone();
    assert_eq!(&sequence[..7], &[0, 1, 2, 3, 1, 2, 3]);
    assert_eq!(sequence.last(), Some(&4));
    assert!(
        sequence[..sequence.len() - 1]
            .iter()
            .all(|&frame| frame < 4),
        "segment end frame was decoded before cancellation: {sequence:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_natural_loop_event_fires_at_each_eof_boundary() {
    let dir = temp_game_dir("video-b4-loop-event-test");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    es.spawn_video_playback(
        id,
        Box::new(IndexedVideoSource::new(
            2,
            120.0,
            30,
            Duration::ZERO,
            Arc::new(Mutex::new(Vec::new())),
        )),
        true,
        0,
        Arc::new(Mutex::new(PlaybackControl::default())),
        Arc::new(Mutex::new(VideoEvents::default())),
    );

    wait_until(Duration::from_secs(2), || es.video_take_event("loop"));
    assert!(
        !es.video_take_event("loop"),
        "a loop boundary must produce one latch"
    );
    wait_until(Duration::from_secs(2), || es.video_take_event("loop"));

    es.video_stop();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_event_registration_and_cancel_clear_stale_latches() {
    let dir = temp_game_dir("video-b4-event-clear-test");
    write_test_avi_2frames(&dir, "clip.avi");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    es.video_open(id, "clip.avi", false).unwrap();
    let events = es.videos.prepared.as_ref().unwrap().events.clone();

    events.lock().unwrap().signal(VideoEventKind::Period);
    events.lock().unwrap().signal(VideoEventKind::SegLoop);
    es.video_set_period_event(1);
    es.video_set_segment_loop(0, 2).unwrap();
    assert!(!es.video_take_event("period"));
    assert!(!es.video_take_event("segLoop"));

    events.lock().unwrap().signal(VideoEventKind::Period);
    events.lock().unwrap().signal(VideoEventKind::SegLoop);
    es.video_cancel_period_event();
    es.video_cancel_segment_loop();
    assert!(!es.video_take_event("period"));
    assert!(!es.video_take_event("segLoop"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_pause_freezes_display_and_events_until_resume() {
    let dir = temp_game_dir("video-b4-pause-test");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    let events = Arc::new(Mutex::new(VideoEvents::default()));
    es.spawn_video_playback(
        id,
        Box::new(IndexedVideoSource::new(
            2,
            120.0,
            20,
            Duration::ZERO,
            Arc::new(Mutex::new(Vec::new())),
        )),
        true,
        0,
        Arc::new(Mutex::new(PlaybackControl::default())),
        events.clone(),
    );
    wait_until(Duration::from_secs(2), || {
        events.lock().unwrap().displayed_count >= 2
    });

    es.video_pause();
    es.video_seek_position(1500.0 / 120.0).unwrap();
    let current = es.videos.current.as_ref().unwrap();
    assert_eq!(current.control.lock().unwrap().pending_seek, Some(1));
    let before = {
        let events = events.lock().unwrap();
        (
            events.displayed_count,
            events.loop_count,
            events.period_count,
            events.seg_loop_count,
        )
    };
    std::thread::sleep(Duration::from_millis(80));
    let during = {
        let events = events.lock().unwrap();
        (
            events.displayed_count,
            events.loop_count,
            events.period_count,
            events.seg_loop_count,
        )
    };
    assert_eq!(during, before, "pause advanced a frame or video event");

    es.video_resume();
    wait_until(Duration::from_secs(2), || {
        events.lock().unwrap().displayed_count > before.0
    });
    es.video_stop();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn stale_video_worker_cannot_draw_or_signal_into_new_playback() {
    let dir = temp_game_dir("video-b4-generation-test");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    let old_control = Arc::new(Mutex::new(PlaybackControl::default()));
    old_control.lock().unwrap().period_frame = Some(0);
    let old_events = Arc::new(Mutex::new(VideoEvents::default()));

    es.spawn_video_playback(
        id,
        Box::new(IndexedVideoSource::new(
            1,
            30.0,
            10,
            Duration::from_millis(80),
            Arc::new(Mutex::new(Vec::new())),
        )),
        false,
        0,
        old_control,
        old_events.clone(),
    );
    let old_playing = es.videos.current.as_ref().unwrap().playing.clone();
    std::thread::sleep(Duration::from_millis(5));

    es.spawn_video_playback(
        id,
        Box::new(IndexedVideoSource::new(
            1,
            120.0,
            200,
            Duration::ZERO,
            Arc::new(Mutex::new(Vec::new())),
        )),
        false,
        0,
        Arc::new(Mutex::new(PlaybackControl::default())),
        Arc::new(Mutex::new(VideoEvents::default())),
    );
    wait_until(Duration::from_secs(2), || !es.video_is_playing());
    wait_until(Duration::from_secs(2), || {
        !old_playing.load(Ordering::Relaxed)
    });

    let scene = es.scene.lock().unwrap();
    let pixmap = scene
        .layer(Page::Fore, id)
        .unwrap()
        .pixmap
        .as_ref()
        .unwrap();
    assert_eq!(pixmap.data, [200, 0, 0, 255]);
    drop(scene);
    assert_eq!(old_events.lock().unwrap().period_count, 0);
    assert!(!es.video_take_event("period"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_playback_uses_each_frame_dimensions() {
    let dir = temp_game_dir("video-changing-frame-size-test");
    let es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    let playing = Arc::new(AtomicBool::new(true));

    playback_loop(
        es.scene.clone(),
        es.wake.clone(),
        id,
        Box::new(ChangingFrameSource::new()),
        false,
        0,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        Arc::new(Mutex::new(PlaybackControl::default())),
        Arc::new(Mutex::new(VideoEvents::default())),
        Arc::new(AtomicU64::new(1)),
        1,
        playing.clone(),
    );

    assert!(!playing.load(Ordering::Relaxed));
    let scene = es.scene.lock().unwrap();
    for page in [Page::Fore, Page::Back] {
        let pixmap = scene.layer(page, id).unwrap().pixmap.as_ref().unwrap();
        assert_eq!((pixmap.width, pixmap.height), (3, 2));
        assert_eq!(pixmap.data, [0, 255, 0, 255].repeat(6));
    }
    drop(scene);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_playback_clears_playing_when_worker_panics() {
    let (wake_tx, _wake_rx) = unbounded();
    let playing = Arc::new(AtomicBool::new(true));
    let worker_playing = playing.clone();

    let result = std::thread::spawn(move || {
        playback_loop(
            Arc::new(Mutex::new(Scene::new(1, 1))),
            WakeSink::Channel(wake_tx),
            1,
            Box::new(PanickingVideoSource),
            false,
            0,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(PlaybackControl::default())),
            Arc::new(Mutex::new(VideoEvents::default())),
            Arc::new(AtomicU64::new(1)),
            1,
            worker_playing,
        )
    })
    .join();

    assert!(result.is_err());
    assert!(!playing.load(Ordering::Relaxed));
}

/// [pausevideo]/[resumevideo]/[rewindvideo] の状態遷移: 再生開始→pause→resume→rewind→stop。
#[test]
fn video_play_prepared_then_pause_resume_rewind_stop_transitions() {
    let dir = temp_game_dir("video-pause-test");
    write_test_avi_5frames(&dir, "clip.avi");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    es.video_prepare(id, "clip.avi", true).unwrap();
    es.video_play_prepared(id).unwrap();

    let mut waited = 0;
    while !es.video_is_playing() && waited < 500 {
        std::thread::sleep(Duration::from_millis(10));
        waited += 10;
    }
    assert!(es.video_is_playing(), "再生開始後は isPlaying のはず");
    assert!(!es.video_is_paused(), "開始直後は isPaused=false のはず");

    es.video_pause();
    assert!(
        es.video_is_paused(),
        "pause 直後は isPaused のはず（フラグは同期的に立つ）"
    );
    std::thread::sleep(Duration::from_millis(80));
    assert!(
        es.video_is_playing(),
        "pause 中もスレッドは生存し isPlaying=true のまま"
    );

    es.video_resume();
    assert!(!es.video_is_paused(), "resume 後は isPaused=false のはず");

    es.video_rewind(); // 巻き戻し要求がパニックせず受理されることを確認する

    es.video_stop();
    let mut waited = 0;
    while es.video_is_playing() && waited < 2000 {
        std::thread::sleep(Duration::from_millis(20));
        waited += 20;
    }
    assert!(
        !es.video_is_playing(),
        "stop 後はスレッドが終了して isPlaying=false になるはず"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_play_prepared_errors_when_nothing_prepared() {
    let dir = temp_game_dir("video-play-prepared-empty-test");
    let mut es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    assert!(
        es.video_play_prepared(id).is_err(),
        "準備済みが無ければエラーになるはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn video_play_prepared_errors_when_layer_mismatch() {
    let dir = temp_game_dir("video-play-prepared-mismatch-test");
    write_test_avi_2frames(&dir, "clip.avi");
    let mut es = make_engine_state(&dir);
    let id_a = es.layer_new("image").unwrap();
    let id_b = es.layer_new("image").unwrap();
    es.video_open(id_a, "clip.avi", false).unwrap();
    assert!(
        es.video_play_prepared(id_b).is_err(),
        "準備済みと異なるレイヤ指定はエラーになるはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// 重い（半透明の全面レイヤが何枚もある）シーンを作り、合成 1 回の所要時間を返す。
fn make_heavy_scene(es: &EngineState) -> Duration {
    let mut scene = Scene::new(2048, 1152);
    for k in 0..6u8 {
        let id = scene.alloc_layer(LayerKind::Image);
        let mut p = Pixmap::new(2048, 1152);
        p.fill([40 * k, 90, 200, 255]);
        let layer = scene.layer_mut(Page::Fore, id).unwrap();
        layer.pixmap = Some(Arc::new(p));
        layer.visible = true;
        layer.opacity = 128;
    }
    let mut bufs = ComposeBufs::new(2048, 1152);
    let mut out = Pixmap::new(2048, 1152);
    let t = Instant::now();
    compose_frame(&scene, Instant::now(), &mut bufs, &mut out);
    let one = t.elapsed();
    *es.scene.lock().unwrap() = scene;
    one
}

/// `screenshot` / `thumb_capture` は、合成・PNG 書き込みのあいだシーンのロックを握らない
/// （握ると描画スレッドと他の ops がその間ずっと待たされる）。別スレッドが `scene.lock()` するときの
/// 最大待ち時間が、合成 1 回ぶんより十分短いことで確かめる。
#[test]
fn screenshot_and_thumb_capture_do_not_hold_the_scene_lock_while_composing() {
    let dir = temp_game_dir("lock-free-capture-test");
    let es = make_engine_state(&dir);
    let compose = make_heavy_scene(&es);
    assert!(
        compose >= Duration::from_millis(5),
        "検証用のシーンが軽すぎる（合成 {compose:?}）"
    );
    let stop = Arc::new(AtomicBool::new(false));
    let observer = {
        let (scene, stop) = (es.scene.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut worst = Duration::ZERO;
            while !stop.load(Ordering::SeqCst) {
                let t = Instant::now();
                drop(scene.lock().unwrap());
                worst = worst.max(t.elapsed());
                std::thread::sleep(Duration::from_micros(200));
            }
            worst
        })
    };
    for _ in 0..4 {
        es.thumb_capture_sized(0, 0).unwrap();
        es.screenshot("r2-lock-test").unwrap();
    }
    stop.store(true, Ordering::SeqCst);
    let worst = observer.join().unwrap();
    eprintln!("SCENE_LOCK_WORST_WAIT_MS={:.2} COMPOSE_MS={:.2}", worst.as_secs_f64() * 1e3, compose.as_secs_f64() * 1e3);
    assert!(
        worst < compose / 3,
        "シーンのロックを最大 {worst:?} 待たされた（合成 1 回 {compose:?}）"
    );
    let _ = savedata::delete_slot(&es.config.id, "r2-lock-test");
    std::fs::remove_dir_all(&dir).ok();
}

/// 巨大な寸法はエラーで返り、その後も同じ状態から動く（`layer_fill` が 40GB を確保して abort しない）。
#[test]
fn oversized_dimensions_are_errors_not_allocations() {
    let dir = temp_game_dir("oversized-dims-test");
    let es = make_engine_state(&dir);
    let id = es.layer_new("image").unwrap();
    assert!(es.layer_fill(id, "fore", 100_000, 100_000, 0, 0, 0, 255).is_err());
    assert!(es.layer_fill(id, "fore", 1 << 31, 1 << 31, 0, 0, 0, 255).is_err());
    assert!(es.layer_msgbox(id, "fore", 100_000, 100_000, 4, 0, 0, 0, 255).is_err());
    assert!(es.thumb_capture_sized(100_000, 100_000).is_err());
    assert!(es.layer_fill(id, "fore", 8, 8, 0, 0, 0, 255).is_ok());
    std::fs::remove_dir_all(&dir).ok();
}

/// 別スレッドがシーンのロックを握ったまま panic して毒化しても、ops は続けて動く
/// （`PoisonError` の unwrap で連鎖して、本当のエラーが埋もれない）。
#[test]
fn a_poisoned_scene_lock_does_not_cascade_into_later_ops() {
    let dir = temp_game_dir("poisoned-lock-test");
    let es = make_engine_state(&dir);
    let scene = es.scene.clone();
    let panicked = std::thread::spawn(move || {
        let _guard = scene.lock().unwrap();
        panic!("シーンのロックを握ったまま落ちる");
    })
    .join();
    assert!(panicked.is_err() && es.scene.is_poisoned());
    let id = es.layer_new("image").unwrap();
    es.layer_fill(id, "fore", 4, 4, 1, 2, 3, 255).unwrap();
    es.layer_set_props(id, "fore", LayerProps { x: Some(3), ..LayerProps::default() }).unwrap();
    assert!(!es.layer_moving());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn next_video_pixmap_reuses_unreferenced_buffer_and_protects_shared_frames() {
    let mut src = noiria_core::codec::video::open_video(FIXTURE_CLIP_5FRAMES.to_vec()).unwrap();
    let mut recycle: Vec<Arc<Pixmap>> = Vec::new();

    let f0 = next_video_pixmap(src.as_mut(), &mut recycle).unwrap().unwrap();
    let f0_data_ptr = f0.data.as_ptr();
    let f0_cap = f0.data.capacity();
    let f0_snapshot = f0.data.clone();
    let mut held_by_scene = f0.clone();
    push_recycle(&mut recycle, f0);

    let f1 = next_video_pixmap(src.as_mut(), &mut recycle).unwrap().unwrap();
    assert!(!Arc::ptr_eq(&held_by_scene, &f1));
    assert_ne!(f1.data.as_ptr(), f0_data_ptr);
    assert_eq!(held_by_scene.data, f0_snapshot);

    held_by_scene = f1.clone();
    push_recycle(&mut recycle, f1);

    let f2 = next_video_pixmap(src.as_mut(), &mut recycle).unwrap().unwrap();
    assert_eq!(f2.data.as_ptr(), f0_data_ptr);
    assert_eq!(f2.data.capacity(), f0_cap);
    assert!(!Arc::ptr_eq(&held_by_scene, &f2));
    assert_ne!(f2.data, f0_snapshot);
}

#[test]
fn layer_load_image_caches_and_shares_arc_pixmap() {
    let dir = temp_game_dir("image-cache-layer-load-test");
    let mut img = Pixmap::new(2, 1);
    img.data[0..4].copy_from_slice(&[255, 0, 0, 255]);
    img.data[4..8].copy_from_slice(&[0, 0, 255, 255]);
    save_png(&img, &dir.join("bg.png")).unwrap();

    let es = make_engine_state(&dir);
    let id1 = es.layer_new("image").unwrap();
    let id2 = es.layer_new("image").unwrap();
    let id3 = es.layer_new("image").unwrap();
    let id4 = es.layer_new("image").unwrap();
    let id5 = es.layer_new("image").unwrap();

    assert_eq!(es.layer_load_image(id1, "fore", "bg.png".into()).unwrap(), (2, 1));
    std::fs::remove_file(dir.join("bg.png")).unwrap();

    assert_eq!(es.layer_load_image(id2, "fore", "bg.png".into()).unwrap(), (2, 1));
    assert_eq!(
        es.layer_load_image_ex(id3, "fore", "bg.png".into(), ImageLoadOpts::default())
            .unwrap(),
        (2, 1)
    );
    assert_eq!(
        es.layer_load_image_ex(
            id4,
            "fore",
            "bg.png".into(),
            ImageLoadOpts {
                flip_h: true,
                ..Default::default()
            }
        )
        .unwrap(),
        (2, 1)
    );
    es.layer_blit_image(id5, "fore", "bg.png", 0, 0, None).unwrap();

    let scene = es.scene.lock().unwrap();
    let p1 = scene.layer(Page::Fore, id1).unwrap().pixmap.as_ref().unwrap();
    let p2 = scene.layer(Page::Fore, id2).unwrap().pixmap.as_ref().unwrap();
    let p3 = scene.layer(Page::Fore, id3).unwrap().pixmap.as_ref().unwrap();
    let p4 = scene.layer(Page::Fore, id4).unwrap().pixmap.as_ref().unwrap();
    let p5 = scene.layer(Page::Fore, id5).unwrap().pixmap.as_ref().unwrap();

    assert!(Arc::ptr_eq(p1, p2));
    assert!(Arc::ptr_eq(p1, p3));
    assert!(!Arc::ptr_eq(p1, p4));
    assert_eq!(&p1.data[0..4], &[255, 0, 0, 255]);
    assert_eq!(&p4.data[0..4], &[0, 0, 255, 255]);
    assert_eq!(&p4.data[4..8], &[255, 0, 0, 255]);
    assert_eq!(p5.data, p1.data);
    drop(scene);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn layer_load_image_evicts_lru_when_images_cache_exceeds_capacity() {
    let dir = temp_game_dir("image-cache-evict-layer-test");
    for name in ["a.png", "b.png", "c.png"] {
        let mut p = Pixmap::new(8, 8);
        p.fill([10, 20, 30, 255]);
        save_png(&p, &dir.join(name)).unwrap();
    }

    let es = make_engine_state(&dir);
    es.ui_cache.lock().unwrap().images = crate::script::ui_cache::PixmapLru::new(2 * 8 * 8 * 4);
    let id = es.layer_new("image").unwrap();

    es.layer_load_image(id, "fore", "a.png".into()).unwrap();
    let a_first = es
        .scene
        .lock()
        .unwrap()
        .layer(Page::Fore, id)
        .unwrap()
        .pixmap
        .clone()
        .unwrap();
    es.layer_load_image(id, "fore", "b.png".into()).unwrap();
    es.layer_load_image(id, "fore", "a.png".into()).unwrap();
    es.layer_load_image(id, "fore", "c.png".into()).unwrap();

    let mut cache = es.ui_cache.lock().unwrap();
    assert_eq!(cache.images.len(), 2);
    assert_eq!(cache.images.bytes(), 2 * 8 * 8 * 4);
    assert!(cache.images.get("b.png").is_none());
    let a_cached = cache.images.get("a.png").unwrap();
    assert!(Arc::ptr_eq(&a_first, &a_cached));
    assert!(cache.images.get("c.png").is_some());
    drop(cache);
    std::fs::remove_dir_all(&dir).ok();
}
