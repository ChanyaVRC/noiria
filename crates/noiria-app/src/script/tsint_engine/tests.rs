use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::unbounded;
use noiria_core::assets::GameDir;
use noiria_core::audio::AudioShared;
use noiria_core::scene::Scene;
use tokio::sync::Notify;

use crate::bridge::{InputEvent, InputSync, PendingWaits, SharedScene, TickSync, WakeSink};
use crate::clock::Clock;

use super::*;

#[test]
fn normalize_game_rel_resolves_relative_and_clamps_at_root() {
    assert_eq!(normalize_game_rel("", "main.ts"), "main.ts");
    assert_eq!(
        normalize_game_rel("sub/main.ts", "./util.ts"),
        "sub/util.ts"
    );
    assert_eq!(normalize_game_rel("a/b/c.ts", "../d.ts"), "a/d.ts");
    // ルートより上へは脱出できない（過剰な `..` は単に無視される）。
    assert_eq!(
        normalize_game_rel("a.ts", "../../../etc/passwd"),
        "etc/passwd"
    );
}

#[test]
fn tsint_can_parse_all_embedded_runtime_modules() {
    // V8 側 (`loader.rs::runtime_modules_all_transpile`) の tsint 版。
    // deno_ast の transpile ではなく tsint 自身のパーサで構文エラーが
    // 無いことを確認する（型注記/interface 等は読み飛ばされる想定）。
    for (name, src) in RUNTIME_MODULES {
        let result = tsint::parse_module(src, name);
        assert!(result.is_ok(), "{name} のパースに失敗: {:?}", result.err());
    }
}

/// テスト専用の使い捨てディレクトリ（drop 時に自動削除）。
struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{tag}-{id}"));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn setup_game_dir(main_ts: &str) -> (GameDir, noiria_core::assets::GameConfig, TempDir) {
    let dir = TempDir::new("noiria-tsint-test");
    std::fs::write(
        dir.path().join("game.json"),
        r#"{"title":"t","id":"noiria-tsint-test","main":"main.ts"}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("main.ts"), main_ts).unwrap();
    let (game_dir, config) = GameDir::open(dir.path()).unwrap();
    (game_dir, config, dir)
}

/// `spawn`/`run_script` と同じ配線を、実スレッドを起こさずテストスレッド上で
/// 直接駆動する end-to-end テスト。ops の型変換（Value <-> Rust 構造体）が
/// 一通り正しく配線されていることを検証する（V8 側 kag_full.rs 相当の
/// tsint 実 ops 版）。
#[test]
fn tsint_ops_wiring_runs_a_small_script_to_completion() {
    let main_ts = r#"
const ops = (globalThis as any).Deno.core.ops;
const id: number = ops.op_layer_new("image");
ops.op_layer_set_props(id, "fore", { x: 10, y: 20, z: 1, opacity: 200, visible: true });
ops.op_log("info", "hello from tsint");
const t0 = ops.op_now();
if (typeof t0 !== "number") throw new Error("op_now should return a number");
await ops.op_wait(5);
ops.op_quit();
"#;
    let (game_dir, config, _guard) = setup_game_dir(main_ts);
    let scene: SharedScene = Arc::new(Mutex::new(Scene::new(64, 64)));
    let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
    drop(input_tx); // 入力は使わない: チャンネルを即座に閉じておく
    let (user_tx, user_rx) = unbounded();
    let (audio_tx, audio_rx) = unbounded();
    drop(audio_rx);

    let ctx = EngineCtx {
        scene: scene.clone(),
        proxy: WakeSink::Channel(user_tx),
        input_rx,
        trans_notify: Arc::new(Notify::new()),
        audio_tx,
        audio_shared: Arc::new(AudioShared::default()),
        game_dir,
        config,
        start: Instant::now(),
        clock: Clock::Real,
        input_sync: InputSync::new(),
        tick_sync: TickSync::new(),
        pending_waits: PendingWaits::new(),
        op_log: None,
    };

    run_script(ctx).expect("tsint スクリプトの実行に失敗");

    // op_layer_set_props が実際にシーンへ反映されているか確認する。
    let scene = scene.lock().unwrap();
    let layer = scene
        .layer(noiria_core::scene::Page::Fore, 1)
        .expect("レイヤが作られていない");
    assert_eq!(
        (layer.x, layer.y, layer.z, layer.opacity, layer.visible),
        (10, 20, 1, 200, true)
    );
    drop(scene);

    // op_quit -> Wake::quit() が呼ばれ、Quit イベントが飛んでいるはず。
    let mut saw_quit = false;
    while let Ok(ev) = user_rx.try_recv() {
        if matches!(ev, UserEvent::Quit) {
            saw_quit = true;
        }
    }
    assert!(saw_quit, "op_quit が WakeSink へ Quit を送っていない");
}

/// move 系 ops (`op_layer_move`/`op_layer_moving`) の tsint 版配線テスト
/// （V8 側 `script::js_tests::move_tween_reaches_to_value_in_v8` 相当）。
#[test]
fn tsint_move_tween_reaches_to_value() {
    let main_ts = r#"
const ops = (globalThis as any).Deno.core.ops;
const id: number = ops.op_layer_new("image");
ops.op_layer_set_props(id, "fore", { x: 0, y: 0, opacity: 0, visible: true });
ops.op_layer_move(id, "fore", { toX: 100, toOpacity: 255, timeMs: 40 });
await ops.op_wait(80);
if (ops.op_layer_moving()) throw new Error("moving は tween の duration 経過後は false のはず");
ops.op_quit();
"#;
    let (game_dir, config, _guard) = setup_game_dir(main_ts);
    let scene: SharedScene = Arc::new(Mutex::new(Scene::new(64, 64)));
    let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
    drop(input_tx);
    let (user_tx, _user_rx) = unbounded();
    let (audio_tx, audio_rx) = unbounded();
    drop(audio_rx);

    let ctx = EngineCtx {
        scene: scene.clone(),
        proxy: WakeSink::Channel(user_tx),
        input_rx,
        trans_notify: Arc::new(Notify::new()),
        audio_tx,
        audio_shared: Arc::new(AudioShared::default()),
        game_dir,
        config,
        start: Instant::now(),
        clock: Clock::Real,
        input_sync: InputSync::new(),
        tick_sync: TickSync::new(),
        pending_waits: PendingWaits::new(),
        op_log: None,
    };

    run_script(ctx).expect("tsint スクリプトの実行に失敗");

    // 完了した tween の確定は renderer の責務: 1 周期ぶんを模して呼ぶ。
    let mut scene = scene.lock().unwrap();
    let now = Instant::now();
    scene.settle_finished_tweens(now);
    let layer = scene
        .layer(noiria_core::scene::Page::Fore, 1)
        .expect("レイヤが作られていない");
    assert!(layer.tween.is_none(), "settle 後は tween が外れているはず");
    assert_eq!(
        (layer.x, layer.opacity),
        (100, 255),
        "位置/不透明度が to 値になっているはず"
    );
}

/// [move path=]（W5）の tsint 版配線テスト: `op_layer_move_path` へ 2 区間の配列を
/// 渡し、第一区間の tween 開始 + 残り区間の tween_queue 積みが V8 側と同じ Value 変換で
/// 動くことを確認する（区間をまたぐ継ぎ目自体は noiria-core の scene.rs 側で検証済み）。
#[test]
fn tsint_move_path_wiring_starts_first_leg_and_queues_rest() {
    let main_ts = r#"
const ops = (globalThis as any).Deno.core.ops;
const id: number = ops.op_layer_new("image");
ops.op_layer_set_props(id, "fore", { x: 0, y: 0, opacity: 0, visible: true });
ops.op_layer_move_path(id, "fore", [
  { toX: 50, timeMs: 40 },
  { toX: 150, toY: 80, toOpacity: 128, timeMs: 40 },
], false);
ops.op_quit();
"#;
    let (game_dir, config, _guard) = setup_game_dir(main_ts);
    let scene: SharedScene = Arc::new(Mutex::new(Scene::new(64, 64)));
    let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
    drop(input_tx);
    let (user_tx, _user_rx) = unbounded();
    let (audio_tx, audio_rx) = unbounded();
    drop(audio_rx);

    let ctx = EngineCtx {
        scene: scene.clone(),
        proxy: WakeSink::Channel(user_tx),
        input_rx,
        trans_notify: Arc::new(Notify::new()),
        audio_tx,
        audio_shared: Arc::new(AudioShared::default()),
        game_dir,
        config,
        start: Instant::now(),
        clock: Clock::Real,
        input_sync: InputSync::new(),
        tick_sync: TickSync::new(),
        pending_waits: PendingWaits::new(),
        op_log: None,
    };

    run_script(ctx).expect("tsint スクリプトの実行に失敗");

    let scene = scene.lock().unwrap();
    let layer = scene
        .layer(noiria_core::scene::Page::Fore, 1)
        .expect("レイヤが作られていない");
    let t = layer
        .tween
        .as_ref()
        .expect("第一区間の tween が開始しているはず");
    // main_ts で opacity: 0 に設定済みなので、toOpacity 省略時はその現在値 0 を引き継ぐ。
    assert_eq!(
        (t.to_x, t.to_y, t.to_opacity),
        (50, 0, 0),
        "第一区間: y/opacity は現在値を引き継ぐ"
    );
    assert!(t.spline.is_none());
    assert_eq!(layer.tween_queue.len(), 1);
    let wp = &layer.tween_queue[0];
    assert_eq!((wp.x, wp.y, wp.opacity), (150, 80, 128));
    assert!(wp.spline.is_none());
}

/// drive の待機・起床の測定用: 実時間クロックで 1 本のスクリプトを別スレッドで走らせる。
struct DriveRun {
    input_tx: tokio::sync::mpsc::UnboundedSender<InputEvent>,
    wakes: Arc<AtomicU64>,
    finished: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    _dir: TempDir,
}

impl DriveRun {
    fn start(main_ts: &str) -> Self {
        let (game_dir, config, dir) = setup_game_dir(main_ts);
        let scene: SharedScene = Arc::new(Mutex::new(Scene::new(64, 64)));
        let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
        let (user_tx, _user_rx) = unbounded();
        let (audio_tx, audio_rx) = unbounded();
        drop(audio_rx);
        let ctx = EngineCtx {
            scene,
            proxy: WakeSink::Channel(user_tx),
            input_rx,
            trans_notify: Arc::new(Notify::new()),
            audio_tx,
            audio_shared: Arc::new(AudioShared::default()),
            game_dir,
            config,
            start: Instant::now(),
            clock: Clock::Real,
            input_sync: InputSync::new(),
            tick_sync: TickSync::new(),
            pending_waits: PendingWaits::new(),
            op_log: None,
        };
        let wakes = Arc::new(AtomicU64::new(0));
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (w, f) = (wakes.clone(), finished.clone());
        let handle = std::thread::spawn(move || {
            run_script_counted(ctx, Some(&w)).expect("tsint スクリプトの実行に失敗");
            f.store(true, AtomicOrdering::SeqCst);
        });
        Self {
            input_tx,
            wakes,
            finished,
            handle: Some(handle),
            _dir: dir,
        }
    }

    fn wakes(&self) -> u64 {
        self.wakes.load(AtomicOrdering::Relaxed)
    }

    /// 終わるまで待つ。`limit` を超えたら panic。終わるまでの経過時間を返す。
    fn join_within(&mut self, limit: Duration) -> Duration {
        let t = Instant::now();
        while !self.finished.load(AtomicOrdering::SeqCst) {
            assert!(t.elapsed() < limit, "スクリプトが {limit:?} 以内に終わらない");
            std::thread::sleep(Duration::from_millis(1));
        }
        self.handle.take().unwrap().join().unwrap();
        t.elapsed()
    }
}

fn click() -> InputEvent {
    InputEvent {
        kind: "click",
        x: 0.0,
        y: 0.0,
        button: Some("left"),
        key: None,
        delta: None,
        t: 0.0,
    }
}

/// 本番（windowed）の `timeBeginPeriod(1)` 下を模す（待機中の起床が 1ms 刻みになる条件）。
#[cfg(windows)]
fn begin_1ms_timer_period() {
    #[link(name = "winmm")]
    extern "system" {
        fn timeBeginPeriod(period: u32) -> u32;
    }
    // SAFETY: 引数は有効な分解能（ms）。プロセス内で有効なだけで状態を壊さない。
    unsafe {
        timeBeginPeriod(1);
    }
}
#[cfg(not(windows))]
fn begin_1ms_timer_period() {}

/// このプロセスの CPU 時間（カーネル + ユーザー、ms）。待機中の CPU 消費の測定用。
#[cfg(windows)]
fn process_cpu_ms() -> f64 {
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        lo: u32,
        hi: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetProcessTimes(h: isize, c: *mut FileTime, e: *mut FileTime, k: *mut FileTime, u: *mut FileTime) -> i32;
    }
    let (mut c, mut e, mut k, mut u) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
    // SAFETY: 4 つとも有効な書き込み先。現在プロセスのハンドルは疑似ハンドルで閉じる必要がない。
    unsafe {
        GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
    }
    let t = |f: FileTime| ((u64::from(f.hi) << 32) | u64::from(f.lo)) as f64 / 1e4;
    t(k) + t(u)
}
#[cfg(not(windows))]
fn process_cpu_ms() -> f64 {
    0.0
}

const WAIT_INPUT_THEN_QUIT: &str = r#"
const ops = (globalThis as any).Deno.core.ops;
await ops.op_input_next_event();
ops.op_quit();
"#;

/// クリック待ちの間、drive が 1 秒に何回起きるか（旧実装は 1〜2ms スリープで永久に再ポーリング）。
#[test]
fn drive_sleeps_while_waiting_for_input() {
    begin_1ms_timer_period();
    let mut run = DriveRun::start(WAIT_INPUT_THEN_QUIT);
    std::thread::sleep(Duration::from_millis(1500)); // 起動（モジュール評価）を済ませる
    let before = run.wakes();
    let cpu_before = process_cpu_ms();
    std::thread::sleep(Duration::from_millis(1000));
    let idle_wakes = run.wakes() - before;
    eprintln!("DRIVE_IDLE_WAKES_PER_SEC={idle_wakes} DRIVE_IDLE_CPU_MS_PER_SEC={:.1}", process_cpu_ms() - cpu_before);
    let sent = Instant::now();
    run.input_tx.send(click()).unwrap();
    let latency = run.join_within(Duration::from_secs(2));
    eprintln!("DRIVE_INPUT_LATENCY_MS={}", latency.as_millis());
    assert!(sent.elapsed() < Duration::from_secs(2));
    assert!(idle_wakes <= 10, "入力待ちで 1 秒に {idle_wakes} 回起きている（眠っていない）");
    assert!(latency < Duration::from_millis(100), "入力から再開まで {latency:?}");
}
