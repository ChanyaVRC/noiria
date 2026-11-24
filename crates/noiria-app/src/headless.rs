//! --autotest: ウィンドウも音も出さないヘッドレス実行モード。
//! 一定間隔で合成クリック/キーを送ってシナリオを自動進行させ、
//! 合成フレームを PNG として書き出す。CI のスモークテストにも使う。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossbeam_channel::Receiver;
use noiria_core::assets::save_png;
use noiria_core::compose::{compose_frame, ComposeBufs};
use noiria_core::pixmap::Pixmap;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Notify;

use crate::autotest_script::{Button, Runner, Step};
use crate::bridge::{
    InputEvent, InputSync, LockExt, PendingWaits, SharedScene, TickSync, UserEvent,
};
use crate::clock::Clock;

/// フレームループ 1 周ぶんの仮想時刻の刻み幅。実際の `thread::sleep` 時間の
/// ブレとは独立に、この固定値だけ `clock` を進める（`crate::clock` のドキュメント参照）。
const TICK: Duration = Duration::from_millis(16);

/// ロックステップ同期の ack 待ちに許す実時間の上限。デッドロック安全性のための
/// 保険（正常時は数ミリ秒〜十数ミリ秒で ack が届く想定）。超過してもハング
/// させず、警告ログを出して進行する（`docs/implementation-notes.md`
/// 「2026-07-13 headless ロックステップ完全決定論化」参照）。
const ACK_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) struct HeadlessParams {
    pub(crate) scene: SharedScene,
    pub(crate) input_tx: UnboundedSender<InputEvent>,
    pub(crate) trans_notify: Arc<Notify>,
    pub(crate) user_rx: Receiver<UserEvent>,
    pub(crate) start: Instant,
    /// `main.rs` が作る仮想クロック（`EngineCtx.clock` と同じインスタンスを共有する）。
    /// このフレームループだけが `advance` を呼んで進める。
    pub(crate) clock: Clock,
    /// `main.rs` が作るロックステップ同期カウンタ（`EngineCtx.input_sync` と同じ
    /// インスタンスを共有する）。
    pub(crate) input_sync: InputSync,
    /// `main.rs` が作る tick ドレインバリア用カウンタ（`EngineCtx.tick_sync` と同じ
    /// インスタンスを共有する。`crate::bridge::TickSync` 参照）。
    pub(crate) tick_sync: TickSync,
    /// 実際に起動されているエンジンが tsint の場合のみ `true`。tsint 実行時だけ
    /// 本フィールドによるバリアを使う（`TickSync` のドキュメント参照）。V8 は
    /// `pending_waits`（in-flight な仮想 `op_wait` の有無）で別途バリアの要否を
    /// 判定する（`pending_waits` フィールドのドキュメント参照）。
    pub(crate) tick_barrier: bool,
    /// `main.rs` が作る、V8 専用の `op_wait` 仮想 in-flight カウンタ
    /// （`EngineCtx.pending_waits` と同じインスタンスを共有する。
    /// `crate::bridge::PendingWaits` 参照）。tsint はこれに触れないため常に 0 のまま
    /// （tsint 実行時は `tick_barrier` 側のバリアだけが効く）。
    pub(crate) pending_waits: PendingWaits,
    pub(crate) duration: Duration,
    pub(crate) out_dir: PathBuf,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// `--autotest-script` の展開済みステップ。`Some` なら固定振付の代わりにこれを実行する。
    pub(crate) script: Option<Vec<Step>>,
    /// `--autotest-fast`: フレーム毎の実時間スリープを省き仮想時間だけ進める（スクリプトモード用）。
    pub(crate) fast: bool,
}

/// 入力イベント送信直後、スクリプト側が「次の入力待ちへ戻った」ことを ack するまで
/// 実時間でブロックする。送信〜ack の間は `p.clock.advance()` を呼ばない
/// （呼び出し側の責務）ことで、スクリプト側がそのイベントを処理する間に読む
/// `Clock::now()` の値を headless 側が送信時に使った値と厳密に一致させる。
///
/// デッドロック安全性: スクリプトスレッドが（バグ等で）二度と入力待ちへ戻らない
/// 場合に備えて `ACK_TIMEOUT` で必ず打ち切る。トランジション/tween 進行待ちは
/// スクリプト側の入力ポンプとは別の非同期フローなので、通常この待ちは
/// 数ミリ秒以内に解消する。
fn wait_for_ack(sync: &InputSync, gen_before: u64) {
    if !sync.wait_after(gen_before, ACK_TIMEOUT) {
        log::warn!(
            "入力イベントの ACK がタイムアウトしました（スクリプト側が入力待ちへ\
             戻っていません）。仮想クロックを進めて続行します"
        );
    }
}

/// tick ドレインバリア（tsint 専用、`crate::bridge::TickSync` 参照）:
/// `p.clock.advance()` の直後に呼ぶ。スクリプトスレッドがこの advance を観測し、
/// その仮想時刻で処理すべきタイマー/オーディオ猶予/入力を全て処理し切って
/// アイドルへ戻る（＝ `target_gen` 以上を `mark_complete` する）まで実時間で
/// ブロックする。
///
/// デッドロック安全性: `wait_for_ack` と同じ `ACK_TIMEOUT` を使い、
/// 超過してもハングさせず警告ログを出して進行する。
fn wait_for_tick_drain(sync: &TickSync, target_gen: u64) {
    if !sync.wait_complete(target_gen, ACK_TIMEOUT) {
        log::warn!(
            "tick ドレインバリアがタイムアウトしました（スクリプト側が仮想 tick {target_gen} \
             までの処理を完了していません）。続行します"
        );
    }
}

/// 自動進行のクリック位置。送りクリック（中央）と、選択肢リンクを
/// 拾うための左寄り縦走査を交互に行う。
fn choreography_click(step: usize) -> (f64, f64) {
    // メッセージウィンドウ top=480, marginY=18, 行送り 41.6px:
    // 行1 (選択肢の 1 個目) ≈ y 540-581, 行2 ≈ y 581-623。
    const POS: [(f64, f64); 8] = [
        (640.0, 600.0),
        (160.0, 560.0),
        (640.0, 540.0),
        (160.0, 620.0),
        (640.0, 620.0),
        (160.0, 600.0),
        (200.0, 560.0),
        (200.0, 620.0),
    ];
    POS[step % POS.len()]
}

/// 仮想クロックを 1 tick 進め、スクリプト側がその時刻の処理を終えるまで待つ（固定振付と
/// スクリプトモードの共通部分。順序の根拠は下記コメント）。
fn advance_tick(p: &HeadlessParams) {
    // tick ドレインバリア: 次の周回の compose_frame/スクリーンショットが、この
    // advance を「スクリプト側が処理し切った」後の状態で行われるようにする
    // （`wait_for_tick_drain`/`TickSync` のドキュメント参照）。
    //
    // V8（`pending_waits > 0`、＝仮想 `op_wait` が in-flight）: `request()` を
    // `clock.advance()` より**前**に呼ぶ。V8 の `run_script` ポーリングループは
    // wake 駆動（`Clock::advance` 内の watch `send` で起こされたときしか
    // 再ポーリングされない）ため、この順序でないと「advance の watch 通知で
    // 起きた時点でまだ新しい要求世代が見えていない」レースになりうる
    // （`TickSync` ドキュメントの happens-before の節を参照。tsint とは逆順で、
    // 変更しないこと）。`pending_waits == 0`（アイドル tick、V8 が入力待ちのみ）
    // では本バリアを丸ごとスキップする: 誰も watch を起こさないため、無条件に
    // 張ると毎 tick `ACK_TIMEOUT`（3秒）に到達してしまう（`PendingWaits`
    // ドキュメント参照）。
    //
    // tsint（`tick_barrier`）: 既存どおり `advance()` の**後**に `request()` を
    // 呼ぶ（tsint の `drive()` は busy-loop なのでこの順序で安全。`TickSync`
    // ドキュメント参照）。V8 と tsint は排他（tsint は `pending_waits` に
    // 触れないため常に 0）なので同じ `tick_sync` を共有していても衝突しない。
    //
    // V8 は `request_for(expected_reactors)` を使う: 同時に複数の `op_wait` が
    // in-flight のとき、最初の 1 個の反応だけで「この tick は完了」と誤認しない
    // ように、`advance()` 直前に読んだ in-flight 数ぶんの `TickSync::react()` が
    // 揃うまで `mark_complete` されない（`TickSync::request_for`/`ops_sys.rs::op_wait`
    // のドキュメント参照）。
    let pending = p.pending_waits.get();
    if pending > 0 {
        let gen = p.tick_sync.request_for(pending);
        p.clock.advance(TICK);
        wait_for_tick_drain(&p.tick_sync, gen);
    } else {
        p.clock.advance(TICK);
        if p.tick_barrier {
            let gen = p.tick_sync.request();
            wait_for_tick_drain(&p.tick_sync, gen);
        }
    }
}

fn now_ms(p: &HeadlessParams) -> f64 {
    p.clock.now().duration_since(p.start).as_secs_f64() * 1000.0
}

/// 入力イベントを送信し、スクリプト側の ack（直前イベントの同期処理完了）を待ってから返る。
fn send_input(p: &HeadlessParams, ev: InputEvent) {
    let gen_before = p.input_sync.generation();
    let _ = p.input_tx.send(ev);
    p.tick_sync.notify_script();
    wait_for_ack(&p.input_sync, gen_before);
}

fn drain_quit(p: &HeadlessParams) -> bool {
    let mut quit = false;
    while let Ok(ev) = p.user_rx.try_recv() {
        quit |= matches!(ev, UserEvent::Quit);
    }
    quit
}

fn step_scene_animations(p: &HeadlessParams, scene: &mut noiria_core::scene::Scene, now: Instant) {
    if scene
        .transition
        .as_ref()
        .is_some_and(|t| t.is_finished(now))
    {
        scene.swap_after_transition();
        p.trans_notify.notify_waiters();
        p.tick_sync.notify_script();
    }
    scene.settle_finished_tweens(now);
}

pub(crate) fn run_headless(mut p: HeadlessParams) -> Result<()> {
    if let Some(steps) = p.script.take() {
        return run_script(p, steps);
    }
    std::fs::create_dir_all(&p.out_dir)?;
    let mut bufs = ComposeBufs::new(p.width, p.height);
    let mut frame = Pixmap::new(p.width, p.height);

    let end = p.start + p.duration;
    let mut last_shot = p.clock.now();
    let mut first_shot_done = false;
    let mut shot_no = 0u32;
    let mut last_click = p.clock.now();
    let mut click_step = 0usize;
    let mut sent_save = false;
    let mut sent_load = false;
    let mut sent_menu_open = false;
    let mut sent_menu_close = false;
    let mut sent_auto = false;
    let mut sent_auto_off = false;
    // move 送信の 300ms 後に撮る追加ショット（ホバーハイライト検証用）
    let mut hover_shot_at: Option<Instant> = None;

    let send_click = |p: &HeadlessParams, x: f64, y: f64| {
        send_input(
            p,
            InputEvent {
                kind: "click",
                x,
                y,
                button: Some("left"),
                key: None,
                delta: None,
                t: now_ms(p),
            },
        );
    };
    let send_key = |p: &HeadlessParams, k: &str| {
        send_input(
            p,
            InputEvent {
                kind: "key",
                x: 0.0,
                y: 0.0,
                button: None,
                key: Some(k.to_string()),
                delta: None,
                t: now_ms(p),
            },
        );
    };

    log::info!(
        "ヘッドレス自動テスト開始: {}秒間, 出力先 {}",
        p.duration.as_secs(),
        p.out_dir.display()
    );

    // 起動同期ゲート: スクリプトスレッドが最初に「入力待ち」へ到達する（= 起動時の
    // 同期初期化処理を終える）まで仮想クロックは 0 のまま据え置く。これにより
    // 起動直後の演出（[trans]/tween 等）の開始時刻としてスクリプト側が読む
    // `Clock::now()` が run ごとにブレない（`wait_for_ack` と同じデッドロック安全性:
    // `ACK_TIMEOUT` で打ち切る）。
    wait_for_ack(&p.input_sync, 0);

    loop {
        if p.clock.now() >= end {
            log::info!("自動テスト時間満了");
            break;
        }
        if drain_quit(&p) {
            log::info!("スクリプトから終了要求を受信");
            break;
        }

        let now = p.clock.now();
        {
            let mut scene = p.scene.lock_unpoisoned();
            step_scene_animations(&p, &mut scene, now);
            compose_frame(&scene, now, &mut bufs, &mut frame);
        }

        // スクリーンショット: 起動1.5秒後に1枚、以降2.5秒毎
        let shot_due = if !first_shot_done {
            now.duration_since(p.start) >= Duration::from_millis(1500)
        } else {
            now.duration_since(last_shot) >= Duration::from_millis(2500)
        };
        let hover_due = hover_shot_at.is_some_and(|t| now >= t);
        if shot_due || hover_due {
            first_shot_done = true;
            last_shot = now;
            let suffix = if hover_due { "_hover" } else { "" };
            if hover_due {
                hover_shot_at = None;
            }
            let path = p.out_dir.join(format!("frame_{shot_no:02}{suffix}.png"));
            if let Err(e) = save_png(&frame, &path) {
                log::warn!("スクリーンショット保存失敗: {e:#}");
            }
            shot_no += 1;
        }

        // 自動進行: 2 秒後から 700ms 毎にクリック
        if now.duration_since(p.start) >= Duration::from_secs(2)
            && now.duration_since(last_click) >= Duration::from_millis(700)
        {
            last_click = now;
            // クイックセーブ/ロードの動作確認を織り込む
            let elapsed = now.duration_since(p.start).as_secs();
            if !sent_auto && elapsed >= 6 {
                sent_auto = true;
                send_key(&p, "a");
            } else if sent_auto && !sent_auto_off && elapsed >= 8 {
                sent_auto_off = true;
                send_key(&p, "a");
            } else if !sent_save && elapsed >= 9 {
                sent_save = true;
                send_key(&p, "F5");
            } else if !sent_load && elapsed >= 14 {
                sent_load = true;
                send_key(&p, "F9");
            } else if !sent_menu_open && elapsed >= 18 {
                sent_menu_open = true;
                send_key(&p, "Escape");
            } else if sent_menu_open && !sent_menu_close && elapsed >= 21 {
                sent_menu_close = true;
                send_key(&p, "Escape");
            } else {
                // move → (次tick) click の交互。ホバーハイライトも検証する
                let (x, y) = choreography_click(click_step / 2);
                if click_step.is_multiple_of(2) {
                    send_input(
                        &p,
                        InputEvent {
                            kind: "move",
                            x,
                            y,
                            button: None,
                            key: None,
                            delta: None,
                            t: now_ms(&p),
                        },
                    );
                    hover_shot_at = Some(now + Duration::from_millis(300));
                } else {
                    send_click(&p, x, y);
                }
                click_step += 1;
            }
        }

        std::thread::sleep(TICK);
        advance_tick(&p);
    }

    // 最終フレーム
    {
        let scene = p.scene.lock_unpoisoned();
        compose_frame(&scene, p.clock.now(), &mut bufs, &mut frame);
    }
    let path = p.out_dir.join("frame_final.png");
    save_png(&frame, &path)?;
    log::info!("自動テスト完了: {} 枚のフレームを保存", shot_no + 1);
    Ok(())
}

/// `--autotest-script`: 展開済み DSL ステップを仮想時間で実行する。固定振付と同じ
/// ロックステップ契約（イベント送信ごとに ack を待ち、その間クロックを進めない）。
///
/// 固定振付との違い: ① 毎 tick の合成・自動撮影はしない（`shot` の時だけ合成する。合成は
/// `&Scene` 読み取りのみで状態に影響しない）② `fast` なら実時間スリープを省く
/// ③ スクリプトの致命終了は終了コード非 0（固定振付は従来どおり 0 のまま）。
fn run_script(p: HeadlessParams, steps: Vec<Step>) -> Result<()> {
    std::fs::create_dir_all(&p.out_dir)?;
    let mut bufs = ComposeBufs::new(p.width, p.height);
    let mut frame = Pixmap::new(p.width, p.height);
    let n_steps = steps.len();
    let mut runner = Runner::new(steps);
    let mut shots = 0u32;
    let end = p.start + p.duration;

    let make_ev =
        |kind: &'static str, x: f64, y: f64, button: Option<&'static str>, key: Option<String>| {
            InputEvent {
                kind,
                x,
                y,
                button,
                key,
                delta: None,
                t: now_ms(&p),
            }
        };

    log::info!(
        "スクリプト autotest 開始: {} ステップ, 出力先 {}{}",
        n_steps,
        p.out_dir.display(),
        if p.fast { " (高速)" } else { "" }
    );
    wait_for_ack(&p.input_sync, 0);

    loop {
        let now = p.clock.now();
        if now >= end {
            log::warn!(
                "仮想時間の上限（{}秒）に達したためスクリプトを打ち切ります",
                p.duration.as_secs()
            );
            break;
        }
        if drain_quit(&p) {
            log::info!("スクリプトから終了要求を受信");
            break;
        }

        {
            let mut scene = p.scene.lock_unpoisoned();
            step_scene_animations(&p, &mut scene, now);
        }

        let t_ms = now.duration_since(p.start).as_millis() as u64;
        if runner.finished(t_ms) {
            log::info!("スクリプト終端に到達");
            break;
        }
        while let Some(step) = runner.poll(t_ms) {
            match step {
                Step::Wait(_) => {}
                Step::Move(x, y) => send_input(&p, make_ev("move", x, y, None, None)),
                Step::Click(x, y, b) => {
                    send_input(&p, make_ev("move", x, y, None, None));
                    let button = if b == Button::Left { "left" } else { "right" };
                    send_input(&p, make_ev("click", x, y, Some(button), None));
                }
                Step::Key(k) => send_input(&p, make_ev("key", 0.0, 0.0, None, Some(k))),
                Step::KeyUp(k) => send_input(&p, make_ev("keyup", 0.0, 0.0, None, Some(k))),
                Step::Shot(name) => {
                    {
                        let scene = p.scene.lock_unpoisoned();
                        compose_frame(&scene, p.clock.now(), &mut bufs, &mut frame);
                    }
                    let path = p.out_dir.join(format!("{name}.png"));
                    save_png(&frame, &path)?;
                    shots += 1;
                }
            }
        }

        if !p.fast {
            std::thread::sleep(TICK);
        }
        advance_tick(&p);
    }

    log::info!("スクリプト autotest 完了: {shots} 枚を保存");
    match crate::fatal::take_script_error() {
        Some(msg) => anyhow::bail!("ゲームスクリプトがエラーで終了しました: {msg}"),
        None => Ok(()),
    }
}
