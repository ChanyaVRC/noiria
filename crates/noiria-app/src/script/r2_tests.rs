//! レビュー対応 R2（スクリプト由来の寸法・数値変換・`sys.wait`・再生終了待ち）の回帰テスト。
//! 同じスクリプトを V8 / tsint の両方へ流して、ops 列と結果が一致することを確認する。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver};
use noiria_core::assets::{save_png, GameDir};
use noiria_core::pixmap::Pixmap;
use noiria_core::audio::{channel_index, AudioShared};
use noiria_core::scene::Scene;
use tokio::sync::Notify;

use crate::bridge::{InputEvent, InputSync, PendingWaits, SharedScene, TickSync, UserEvent, WakeSink};
use crate::clock::Clock;
use crate::script::engine_state::SharedOpLog;
use crate::script::EngineCtx;

#[derive(Clone, Copy, Debug)]
enum Engine {
    V8,
    Tsint,
}

const ENGINES: [Engine; 2] = [Engine::V8, Engine::Tsint];

struct Run {
    scene: SharedScene,
    trans_notify: Arc<Notify>,
    audio_shared: Arc<AudioShared>,
    op_log: SharedOpLog,
    user_rx: Receiver<UserEvent>,
    input_tx: tokio::sync::mpsc::UnboundedSender<InputEvent>,
    dir: PathBuf,
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// `main_ts` を実時間クロックで走らせる。`audio` は起動前に仕込む（`pending` な Play 要求など）。
/// ゲームには 4x4 の `assets/solid.png` を置く。
fn start(engine: Engine, main_ts: &str, audio: impl FnOnce(&AudioShared)) -> Run {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("noiria-r2-{engine:?}-{ns}-{n}"));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(
        dir.join("game.json"),
        format!(r#"{{"title":"t","id":"noiria-r2-test-{n}","main":"main.ts"}}"#),
    )
    .unwrap();
    std::fs::write(dir.join("main.ts"), main_ts).unwrap();
    let mut solid = Pixmap::new(4, 4);
    solid.fill([200, 100, 50, 255]);
    save_png(&solid, &dir.join("assets/solid.png")).unwrap();
    let (game_dir, config) = GameDir::open(&dir).unwrap();
    let scene: SharedScene = Arc::new(Mutex::new(Scene::new(64, 64)));
    let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
    let (user_tx, user_rx) = unbounded();
    let (audio_tx, audio_rx) = unbounded();
    drop(audio_rx);
    let audio_shared = Arc::new(AudioShared::default());
    audio(&audio_shared);
    let op_log: SharedOpLog = Arc::new(Mutex::new(Vec::new()));
    let trans_notify = Arc::new(Notify::new());
    let ctx = EngineCtx {
        scene: scene.clone(),
        proxy: WakeSink::Channel(user_tx),
        input_rx,
        trans_notify: trans_notify.clone(),
        audio_tx,
        audio_shared: audio_shared.clone(),
        game_dir,
        config,
        start: Instant::now(),
        clock: Clock::Real,
        input_sync: InputSync::new(),
        tick_sync: TickSync::new(),
        pending_waits: PendingWaits::new(),
        op_log: Some(op_log.clone()),
    };
    // スレッドは join しない（Quit を送ったあとの後始末は他のハーネスと同じく放置）。
    match engine {
        Engine::V8 => drop(crate::script::spawn(ctx)),
        Engine::Tsint => drop(crate::script::tsint_engine::spawn(ctx)),
    }
    Run {
        scene,
        trans_notify,
        audio_shared,
        op_log,
        user_rx,
        input_tx,
        dir,
    }
}

impl Run {
    /// `UserEvent::Quit` が来るまで待つ。来たら経過時間、`limit` を超えたら None。
    fn wait_quit(&self, limit: Duration) -> Option<Duration> {
        let t = Instant::now();
        while t.elapsed() < limit {
            if let Ok(UserEvent::Quit) = self.user_rx.recv_timeout(Duration::from_millis(5)) {
                return Some(t.elapsed());
            }
        }
        None
    }

    /// ログに `line`（`log info <msg>` の完全一致）が出るまで待つ。
    fn wait_log(&self, msg: &str, limit: Duration) -> bool {
        let want = format!("log info {msg}");
        let t = Instant::now();
        while t.elapsed() < limit {
            if self.log().iter().any(|l| l == &want) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    /// `log info <prefix><number>` の number を返す。
    fn logged_number(&self, prefix: &str) -> f64 {
        let want = format!("log info {prefix}");
        self.log()
            .iter()
            .find_map(|l| l.strip_prefix(&want).and_then(|n| n.trim().parse().ok()))
            .unwrap_or_else(|| panic!("`{prefix}` が無い: {:#?}", self.log()))
    }

    fn log(&self) -> Vec<String> {
        self.op_log.lock().unwrap().clone()
    }

    /// 描画ループの代役: 一定間隔で、完了したトランジションの swap と `trans_notify` を行う。
    fn spawn_renderer(&self) -> Arc<std::sync::atomic::AtomicBool> {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (stop2, scene, notify) = (stop.clone(), self.scene.clone(), self.trans_notify.clone());
        std::thread::spawn(move || {
            while !stop2.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(16));
                let finished = {
                    let mut s = scene.lock().unwrap();
                    if s.transition.as_ref().is_some_and(|t| t.is_finished(Instant::now())) {
                        s.swap_after_transition();
                        true
                    } else {
                        false
                    }
                };
                if finished {
                    notify.notify_waiters();
                }
            }
        });
        stop
    }
}

fn run_to_quit(engine: Engine, main_ts: &str) -> Vec<String> {
    let run = start(engine, main_ts, |_| {});
    run.wait_quit(Duration::from_secs(20))
        .unwrap_or_else(|| panic!("{engine:?}: スクリプトが終わらない: {:#?}", run.log()));
    run.log()
}

/// V8 と tsint で ops 列が一致し、各行が `want` を含む（`layer_new` 等の準備行は除く）。
fn both_agree(main_ts: &str) -> Vec<String> {
    let v8 = run_to_quit(Engine::V8, main_ts);
    let ts = run_to_quit(Engine::Tsint, main_ts);
    assert_eq!(v8, ts, "V8 と tsint の ops 列が食い違う");
    v8
}

const NUMERIC_SCRIPT: &str = r#"
const ops = (globalThis as any).Deno.core.ops;
const id = ops.op_layer_new("image");
const tryOp = (name: string, f: () => unknown) => {
  try { f(); } catch (e) { ops.op_log("info", name + " ERR"); }
};
ops.op_layer_set_props(id, "fore", { x: 10.6, y: -10.6, z: NaN, opacity: 127.5, visible: true });
ops.op_layer_set_props(id, "fore", { x: 1e20, y: -1e20, opacity: 300 });
ops.op_layer_set_props(id, "fore", { x: Infinity, opacity: -3 });
ops.op_layer_move(id, "fore", { toX: 2.5, toOpacity: 254.5, timeMs: 400.6, accel: 0 });
ops.op_layer_move(id, "fore", { toX: Infinity, toY: NaN, timeMs: -5 });
ops.op_layer_move(id, "fore", { toX: 5, timeMs: 1e30 });
ops.op_layer_move(id, "fore", { toX: 5 });
ops.op_layer_fill(id, "fore", -1, 2.4, 1.5, 2.5, 255.6, 300);
ops.op_layer_fill(id, "fore", 3.5, NaN, 0, 0, 0, 0);
ops.op_layer_msgbox(id, "fore", 4.6, 2, 1.5, 0, 0, 0, 255);
ops.op_layer_set_crop(id, "fore", 2.6);
ops.op_layer_crop_move(id, "fore", 5.5, 1e30, 0);
ops.op_layer_crop_move(id, "fore", -5, -1, 0);
ops.op_quake(2.6, -2.6, 100.5);
ops.op_trans_begin({ method: "crossfade" });
ops.op_trans_skip();
tryOp("text_print", () => ops.op_text_print(id, [{ text: "a", color: [127.5, 300, -4, NaN], edgeColor: [0.4, 0.5, 0.6, Infinity] }], false, 0));
tryOp("audio_play", () => ops.op_audio_play("bgm", "x.ogg", false, 1, 2.6, 0, false));
tryOp("audio_stop", () => ops.op_audio_stop("bgm", -1));
ops.op_log("info", "DONE");
ops.op_quit();
"#;

/// 小数・負数・NaN・Infinity・巨大値を両エンジンへ流し、ops 列が一致する（同じ丸め規則: 非有限は 0、
/// 四捨五入、対象型の範囲へ飽和）。期待値は規則から手で導いたもの。
#[test]
fn numeric_arguments_follow_one_rule_on_both_engines() {
    let log = both_agree(NUMERIC_SCRIPT);
    let has = |frag: &str| {
        assert!(
            log.iter().any(|l| l.contains(frag)),
            "`{frag}` を含む行が無い: {log:#?}"
        )
    };
    has("layer_set_props 1 fore LayerProps { x: Some(11), y: Some(-11), z: Some(0), opacity: Some(128), visible: Some(true) }");
    has("x: Some(1073741823), y: Some(-1073741823), z: None, opacity: Some(255)");
    has("x: Some(0), y: None, z: None, opacity: Some(0)");
    has("layer_move 1 fore MoveArgs { to_x: Some(3), to_y: None, to_opacity: Some(255), time_ms: 401, accel: 0.0 }");
    has("to_x: Some(0), to_y: Some(0), to_opacity: None, time_ms: 0");
    has("to_x: Some(5), to_y: None, to_opacity: None, time_ms: 4294967295");
    has("layer_fill 1 fore 0x2 rgba=(2,3,255,255)");
    has("layer_fill 1 fore 4x0 rgba=(0,0,0,0)");
    has("layer_msgbox 1 fore 5x2");
    has("layer_set_crop 1 fore 3");
    has("layer_crop_move 1 fore to=6 time_ms=4294967295");
    has("layer_crop_move 1 fore to=0 time_ms=0");
    has("quake ax=3 ay=-3 ms=101");
    has("trans_begin crossfade 0");
    has("color: Some([128, 255, 0, 0])");
    has("edge_color: Some([0, 1, 1, 0])");
    has("audio_play bgm x.ogg loop=false vol=1 fade=3 start=0 paused=false");
    has("audio_stop bgm fade=0");
    has("log info DONE");
}

const DIMENSION_SCRIPT: &str = r#"
const ops = (globalThis as any).Deno.core.ops;
const id = ops.op_layer_new("image");
let n = 0;
const rejects = (f: () => unknown) => {
  n++;
  try { f(); ops.op_log("info", "ACCEPTED " + n); } catch (e) { ops.op_log("info", "REJECTED " + n); }
};
const clip = { x: 0, y: 0, w: 100000, h: 100000 };
rejects(() => ops.op_layer_fill(id, "fore", 100000, 100000, 0, 0, 0, 255));
rejects(() => ops.op_layer_fill(id, "fore", 2147483648, 2147483648, 0, 0, 0, 255));
rejects(() => ops.op_layer_fill(id, "fore", 4294967295, 1, 0, 0, 0, 255));
rejects(() => ops.op_layer_fill(id, "fore", 16384, 16384, 0, 0, 0, 255));
rejects(() => ops.op_layer_msgbox(id, "fore", 100000, 100000, 4, 0, 0, 0, 255));
rejects(() => ops.op_layer_load_image_scaled(id, "fore", "assets/solid.png", 100000, 100000));
rejects(() => ops.op_thumb_capture(100000, 100000));
rejects(() => ops.op_thumb_capture(20000, 20000));
rejects(() => ops.op_layer_blit_image(id, "fore", "assets/solid.png", 0, 0, clip));
rejects(() => ops.op_layer_load_image_ex(id, "fore", "assets/solid.png", { clip }));
ops.op_layer_fill(id, "fore", 8, 8, 1, 2, 3, 255);
ops.op_thumb_capture(64, 64);
ops.op_log("info", "OK-AFTER");
ops.op_quit();
"#;

/// 巨大・ラップする寸法は Err になり（以前は 40GB の確保でプロセスごと abort）、その後も動く。
#[test]
fn oversized_dimensions_are_rejected_on_both_engines() {
    let log = both_agree(DIMENSION_SCRIPT);
    for n in 1..=10 {
        assert!(
            log.iter().any(|l| l == &format!("log info REJECTED {n}")),
            "操作 {n} が拒否されていない: {log:#?}"
        );
    }
    assert!(log.iter().any(|l| l == "log info OK-AFTER"), "{log:#?}");
}

/// `sys.wait` に非有限・負・巨大値を渡してもスクリプトスレッドが落ちない（`Duration::from_secs_f64` の panic）。
#[test]
fn sys_wait_with_non_finite_values_does_not_kill_the_script_thread() {
    let script = r#"
const ops = (globalThis as any).Deno.core.ops;
ops.op_wait(Infinity);
ops.op_wait(1e300);
ops.op_wait(NaN);
ops.op_wait(-Infinity);
ops.op_log("info", "ALIVE");
await ops.op_wait(NaN);
await ops.op_wait(-5);
await ops.op_wait(30);
ops.op_log("info", "AFTER");
ops.op_quit();
"#;
    for engine in ENGINES {
        let run = start(engine, script, |_| {});
        let took = run.wait_quit(Duration::from_secs(20));
        let log = run.log();
        assert!(took.is_some(), "{engine:?}: 終わらない（スレッドが落ちた）: {log:#?}");
        assert!(log.iter().any(|l| l == "log info ALIVE"), "{engine:?}: {log:#?}");
        assert!(log.iter().any(|l| l == "log info AFTER"), "{engine:?}: {log:#?}");
    }
}

const WAIT_BGM_SCRIPT: &str = r#"
const ops = (globalThis as any).Deno.core.ops;
ops.op_log("info", "READY");
const t0 = ops.op_now();
await ops.op_audio_wait_end("bgm");
ops.op_log("info", "WAITED " + Math.round(ops.op_now() - t0));
ops.op_quit();
"#;

fn bgm() -> usize {
    channel_index("bgm").unwrap()
}

/// Play 要求済みでデコード中（まだミキサに無い）の間は `waitEnd` が終わらない。要求が取り下げられる
/// （デコード失敗・Stop・別の要求による上書き＝すべて `pending` が外れる）と終わる。
#[test]
fn wait_end_waits_while_a_play_request_is_decoding_and_ends_when_it_is_dropped() {
    for engine in ENGINES {
        let request = Arc::new(Mutex::new(0u64));
        let r = request.clone();
        let run = start(engine, WAIT_BGM_SCRIPT, move |shared| {
            *r.lock().unwrap() = shared.mark_play_requested(bgm(), 0);
        });
        assert!(run.wait_log("READY", Duration::from_secs(20)), "{engine:?}");
        // 以前の固定猶予（60ms）を十分に超えても終わらない。
        assert!(
            run.wait_quit(Duration::from_millis(400)).is_none(),
            "{engine:?}: デコード中なのに waitEnd が終わった: {:#?}",
            run.log()
        );
        // 失敗した（または取り下げられた）要求は pending が外れ、待ちが終わる。
        let cancelled = Instant::now();
        run.audio_shared
            .cancel_play_request(bgm(), *request.lock().unwrap(), 0);
        assert!(
            run.wait_quit(Duration::from_secs(5)).is_some(),
            "{engine:?}: 取り下げても終わらない: {:#?}",
            run.log()
        );
        assert!(
            cancelled.elapsed() < Duration::from_millis(300),
            "{engine:?}: 取り下げてから {:?}",
            cancelled.elapsed()
        );
    }
}

/// 音声が鳴っていない・要求も無いチャンネルの `waitEnd` は、固定の猶予を待たずにすぐ終わる。
#[test]
fn wait_end_on_an_idle_channel_returns_without_a_grace_period() {
    for engine in ENGINES {
        let run = start(engine, WAIT_BGM_SCRIPT, |_| {});
        assert!(run.wait_quit(Duration::from_secs(20)).is_some(), "{engine:?}");
        let waited = run.logged_number("WAITED ");
        assert!(waited < 30.0, "{engine:?}: 猶予を待っている: {waited}ms");
    }
}

/// 音声スレッドが無い（送信先が閉じている）環境で `audio.play` した要求は、保留のまま残らない。
#[test]
fn play_without_an_audio_thread_does_not_leave_a_pending_request() {
    let script = r#"
const ops = (globalThis as any).Deno.core.ops;
ops.op_audio_play("bgm", "x.ogg", false, 1, 0, 0, false);
const t0 = ops.op_now();
await ops.op_audio_wait_end("bgm");
ops.op_log("info", "WAITED " + Math.round(ops.op_now() - t0));
ops.op_quit();
"#;
    for engine in ENGINES {
        let run = start(engine, script, |_| {});
        assert!(
            run.wait_quit(Duration::from_secs(20)).is_some(),
            "{engine:?}: 音声スレッドの無い play 後の waitEnd が終わらない: {:#?}",
            run.log()
        );
        assert!(run.logged_number("WAITED ") < 30.0, "{engine:?}");
    }
}

/// 実時間クロックでトランジション完了（描画側の `swap_after_transition` + 通知）を取りこぼさず、短い遅れで再開する。
#[test]
fn trans_wait_resumes_promptly_after_the_renderer_completes_the_transition() {
    let script = r#"
const ops = (globalThis as any).Deno.core.ops;
const t0 = ops.op_now();
ops.op_trans_begin({ method: "crossfade", timeMs: 120 });
await ops.op_trans_wait();
ops.op_log("info", "TRANS_MS " + Math.round(ops.op_now() - t0));
ops.op_quit();
"#;
    for engine in ENGINES {
        let run = start(engine, script, |_| {});
        let stop = run.spawn_renderer();
        let done = run.wait_quit(Duration::from_secs(20));
        stop.store(true, Ordering::SeqCst);
        assert!(done.is_some(), "{engine:?}: {:#?}", run.log());
        let ms = run.logged_number("TRANS_MS ");
        assert!((100.0..250.0).contains(&ms), "{engine:?}: {ms}ms");
    }
}

/// 入力待ちの再開（`input_tx` からの送信で起きる）が短い遅れで届く。
#[test]
fn input_event_resumes_a_waiting_script_promptly() {
    let script = r#"
const ops = (globalThis as any).Deno.core.ops;
ops.op_log("info", "READY");
const ev = await ops.op_input_next_event();
ops.op_log("info", "GOT " + ev.kind);
ops.op_quit();
"#;
    for engine in ENGINES {
        let run = start(engine, script, |_| {});
        assert!(run.wait_log("READY", Duration::from_secs(20)), "{engine:?}");
        std::thread::sleep(Duration::from_millis(300));
        let sent = Instant::now();
        run.input_tx
            .send(InputEvent {
                kind: "click",
                x: 1.0,
                y: 2.0,
                button: Some("left"),
                key: None,
                delta: None,
                t: 0.0,
            })
            .unwrap();
        assert!(run.wait_quit(Duration::from_secs(5)).is_some(), "{engine:?}");
        assert!(sent.elapsed() < Duration::from_millis(100), "{engine:?}: {:?}", sent.elapsed());
        assert!(run.log().iter().any(|l| l == "log info GOT click"));
    }
}
