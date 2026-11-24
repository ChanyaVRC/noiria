//! tsint 非同期 ops (`PendingOps`) とイベントループ駆動 (`Intake` / `drive`)。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::UnboundedReceiver;
use tsint::value::Value;
use tsint::vm::{HostPromise, PumpState};
use tsint::Interpreter;

use crate::bridge::InputEvent;
use crate::script::engine_state::EngineState;

use super::convert::{input_event_to_value, quit_event_value};

// 保留中の `HostPromise` を専用スレッドの駆動ループが期限/ポーリングで
// 解決していく。
// =====================================================================

#[derive(Default)]
pub(super) struct PendingOps {
    /// `op_wait` の期限付きタイマー。
    pub(super) timers: Vec<(Instant, HostPromise)>,
    /// `op_trans_wait` の完了待ち。
    pub(super) trans: Vec<HostPromise>,
    /// `op_audio_wait_end` の完了待ち: (チャンネル index, handle)。
    pub(super) audio: Vec<(usize, HostPromise)>,
    /// `op_input_next_event` の保留中ハンドル（同時に 1 個だけという前提は V8 版と同じ）。
    pub(super) input: Option<HostPromise>,
    /// メインスレッドから届いた未消費の入力イベント。
    pub(super) input_buffer: VecDeque<InputEvent>,
    /// 入力チャンネルが閉じた（メインスレッド側の送信口が drop された）。
    pub(super) input_closed: bool,
    /// ヘッドレスのロックステップ同期用: 現在保留中の `input` ハンドルに対して
    /// `EngineState::input_sync.ack()` を出したか。ハンドル登録時に false へ戻し、
    /// `drive()` が「pump が Idle（マイクロタスク消化済み）かつ入力バッファが空」
    /// という静止条件を観測した最初の反復で ack して true にする
    /// （V8 版 `run_script` のイベントループラッパと同じ意味の同期点）。
    pub(super) input_acked: bool,
}


/// ヘッドレス（仮想クロック）で `drive` がアイドルポーリングする上限間隔。
/// headless.rs のフレーム刻み（16ms、`crate::headless::TICK`）より十分小さくして、
/// `--autotest` の move/click の取りこぼし〜処理遅延の上限を抑える（実時間クロックは眠る: `Intake::Forwarded`）。
const IDLE_POLL_MAX: Duration = Duration::from_millis(2);
/// 実時間クロックで、再生終了待ち（`is_playing` はフラグのポーリング）がある間の起床間隔。
const AUDIO_POLL: Duration = Duration::from_millis(16);
/// 実時間クロックで、トランジション完了待ちがある間の安全網の間隔（通常は完了通知で起きる）。
const TRANS_POLL: Duration = Duration::from_millis(50);
/// 何も待っていなくても、未知の起床要因の取りこぼしがあっても止まらないための最長の眠り。
const MAX_SLEEP: Duration = Duration::from_millis(500);

/// 別スレッドから `drive` を起こす呼び鈴（立てたフラグは `wait` が消費するので、`wait` に入る前の `ring` も失われない）。
struct Doorbell {
    flag: std::sync::Mutex<bool>,
    cv: std::sync::Condvar,
}

impl Doorbell {
    fn new() -> Self {
        Self {
            flag: std::sync::Mutex::new(false),
            cv: std::sync::Condvar::new(),
        }
    }

    fn ring(&self) {
        *self.flag.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.cv.notify_one();
    }

    fn wait(&self, timeout: Duration) {
        let mut flag = self.flag.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !*flag {
            flag = self
                .cv
                .wait_timeout(flag, timeout)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        *flag = false;
    }
}

#[derive(Default)]
struct ForwardedInput {
    events: VecDeque<InputEvent>,
    closed: bool,
}

/// `drive` の入力の受け口。
pub(super) enum Intake {
    /// ヘッドレス: 従来どおり `drive` が毎周 `try_recv` する（仮想クロックの決定論を保つため）。
    Direct(UnboundedReceiver<InputEvent>),
    /// 実時間: 入力チャンネルとトランジション完了通知を別スレッドで待ち、`Doorbell` で `drive` を起こす。
    Forwarded(ForwardedIntake),
}

pub(super) struct ForwardedIntake {
    shared: Arc<std::sync::Mutex<ForwardedInput>>,
    bell: Arc<Doorbell>,
    stop: Arc<tokio::sync::Notify>,
    handle: Option<JoinHandle<()>>,
}

impl Intake {
    pub(super) fn forwarded(mut input_rx: UnboundedReceiver<InputEvent>, trans_notify: Arc<tokio::sync::Notify>) -> Self {
        let shared = Arc::new(std::sync::Mutex::new(ForwardedInput::default()));
        let bell = Arc::new(Doorbell::new());
        let stop = Arc::new(tokio::sync::Notify::new());
        let thread_ctx = (shared.clone(), bell.clone(), stop.clone());
        let handle = std::thread::Builder::new()
            .name("noiria-tsint-wake".into())
            .spawn(move || {
                let (shared, bell, stop) = thread_ctx;
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().build() else {
                    shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner).closed = true;
                    bell.ring();
                    return;
                };
                rt.block_on(async {
                    let mut input_open = true;
                    loop {
                        tokio::select! {
                            ev = input_rx.recv(), if input_open => {
                                let mut s = shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                match ev {
                                    Some(ev) => s.events.push_back(ev),
                                    None => {
                                        s.closed = true;
                                        input_open = false;
                                    }
                                }
                                drop(s);
                                bell.ring();
                            }
                            () = trans_notify.notified() => bell.ring(),
                            () = stop.notified() => break,
                        }
                    }
                });
            })
            .ok();
        Intake::Forwarded(ForwardedIntake {
            shared,
            bell,
            stop,
            handle,
        })
    }

    /// 届いている入力を `pending.input_buffer` へ移す。
    fn drain(&mut self, pending: &Rc<RefCell<PendingOps>>) {
        match self {
            Intake::Direct(rx) => loop {
                match rx.try_recv() {
                    Ok(ev) => pending.borrow_mut().input_buffer.push_back(ev),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        pending.borrow_mut().input_closed = true;
                        break;
                    }
                }
            },
            Intake::Forwarded(f) => {
                let mut s = f.shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut p = pending.borrow_mut();
                p.input_buffer.append(&mut s.events);
                if s.closed {
                    p.input_closed = true;
                }
            }
        }
    }

    /// 次に起きるまで眠る。`Direct` は `TickSync` の Condvar で tick 要求/入力通知を即座に受け、
    /// `Forwarded` は期限か呼び鈴まで眠る。
    fn sleep(
        &self,
        timeout: Option<Duration>,
        tick_sync: &crate::bridge::TickSync,
        seen_req: u64,
        seen_wake: u64,
    ) {
        match self {
            Intake::Direct(_) => {
                let t = timeout.map_or(IDLE_POLL_MAX, |t| t.min(IDLE_POLL_MAX));
                tick_sync.wait_for_request_or_wake(
                    seen_req,
                    seen_wake,
                    t.max(Duration::from_millis(1)),
                );
            }
            Intake::Forwarded(f) => f.bell.wait(timeout.unwrap_or(MAX_SLEEP).min(MAX_SLEEP)),
        }
    }
}

impl Drop for ForwardedIntake {
    fn drop(&mut self) {
        self.stop.notify_one();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// 非同期 ops を駆動するイベントループ。V8 版の `js.run_event_loop().await` に相当する。
pub(super) fn drive(
    interp: &mut Interpreter,
    es: &Rc<RefCell<EngineState>>,
    pending: &Rc<RefCell<PendingOps>>,
    mut intake: Intake,
    wakes: Option<&AtomicU64>,
) -> Result<(), String> {
    loop {
        if let Some(w) = wakes {
            w.fetch_add(1, AtomicOrdering::Relaxed);
        }
        // 1) 入力を取り込む前に wake 世代を読んでおき、drain〜sleep の間に届いた通知を取りこぼさない。
        let wake_seen = es.borrow().tick_sync.wake_generation();
        intake.drain(pending);

        // ヘッドレスの tick ドレインバリア（`crate::bridge::TickSync` 参照）:
        // headless.rs 側の要求世代を、これから読む `now` より**先に**読む。
        // `requested_generation()` → `clock.now()` の順で読むことで、この後の
        // `now` が headless の対応する `Clock::advance()` を確実に観測できる
        // （`TickSync` ドキュメントの happens-before の説明を参照。順序を逆にすると
        // この保証が崩れるので変更しないこと）。
        let tick_req = es.borrow().tick_sync.requested_generation();
        // タイマー期限・オーディオ猶予は clock 基準（windowed の Clock::Real なら
        // Instant::now() と同値で挙動不変、ヘッドレスの Clock::Virtual なら仮想 tick
        // に対して決定的）。
        let now = es.borrow().clock.now();
        let mut resolved_any = false;

        // 2) タイマー (op_wait) の期限が来たものを解決する。
        let due: Vec<HostPromise> = {
            let mut p = pending.borrow_mut();
            // 期限が来たものが無い周（ほとんど）は一覧を作り直さない。
            if p.timers.iter().any(|(deadline, _)| *deadline <= now) {
                let (due, remaining): (Vec<_>, Vec<_>) = std::mem::take(&mut p.timers)
                    .into_iter()
                    .partition(|(deadline, _)| *deadline <= now);
                p.timers = remaining;
                due.into_iter().map(|(_, h)| h).collect()
            } else {
                Vec::new()
            }
        };
        for h in due {
            resolved_any = true;
            h.resolve(interp.vm(), Value::Undefined);
        }

        // 3) トランジション完了待ち (op_trans_wait) をポーリングする。
        // 待っている人がいないときはシーンのロックを取らない。
        if !pending.borrow().trans.is_empty() && !es.borrow().trans_active() {
            let waiters: Vec<HostPromise> = std::mem::take(&mut pending.borrow_mut().trans);
            for h in waiters {
                resolved_any = true;
                h.resolve(interp.vm(), Value::Undefined);
            }
        }

        // 4) オーディオ再生終了待ち (op_audio_wait_end) をポーリングする。
        if !pending.borrow().audio.is_empty() {
            let items = std::mem::take(&mut pending.borrow_mut().audio);
            let mut still = Vec::with_capacity(items.len());
            for (ch, h) in items {
                if es.borrow().audio_busy(ch) {
                    still.push((ch, h));
                } else {
                    resolved_any = true;
                    h.resolve(interp.vm(), Value::Undefined);
                }
            }
            pending.borrow_mut().audio = still;
        }

        // 5) 入力イベント (op_input_next_event) を配送する。
        {
            let handle = {
                let mut p = pending.borrow_mut();
                if p.input.is_some() && (!p.input_buffer.is_empty() || p.input_closed) {
                    p.input.take()
                } else {
                    None
                }
            };
            if let Some(h) = handle {
                let ev = pending.borrow_mut().input_buffer.pop_front();
                resolved_any = true;
                match ev {
                    Some(ev) => h.resolve(interp.vm(), input_event_to_value(&ev)),
                    None => h.resolve(interp.vm(), quit_event_value()),
                }
            }
        }

        // 6) マイクロタスクキュー・トップレベル await の継続を汲み尽くす。
        //    `tsint::vm::Vm::run_event_loop` はマイクロタスクキューを常に空になるまで
        //    汲み尽くしてから `PumpState::Idle`/`Waiting` を返す（`pending_host_count`
        //    の残数だけで判定。tsint リポジトリ `src/vm.rs` 参照）。`Waiting` は
        //    「まだ解決されていないホスト Promise（タイマー/オーディオ/入力待ち等）が
        //    残っている」という意味で、`op_input_next_event` が入力待ちハンドルを
        //    登録した直後は常にこれに該当する（そのハンドル自身が未解決の
        //    ホスト Promise としてカウントされるため）。よって「入力ポンプが次の
        //    イベント待ちに入った」瞬間は原理的に必ず `Waiting` になり、`Idle` には
        //    ならない。
        let pump = interp.run_event_loop();

        // 6.5) ヘッドレスのロックステップ同期: 上記の通りマイクロタスクは pump の
        //      戻り値（Idle/Waiting どちらでも）に関わらずこの時点で必ず汲み尽くされて
        //      いるので、`pump` の値では判定しない（`Idle` のみに限定すると、入力待ち
        //      登録直後は常に `Waiting` になるため ack が一切発火しなくなるバグになる）。
        //      入力ポンプが次のイベント待ちに入っている（保留ハンドルあり・バッファ空）
        //      なら、このハンドルに対して一度だけ ack を出す（V8 版 `run_script` の
        //      イベントループラッパと同じ同期点。`docs/implementation-notes.md`
        //      「2026-07-13 tsint tick ドレインバリア」参照）。windowed 実行では誰も
        //      観測しないため挙動不変。`pump` 自体は次段（7）の終了判定で使う。
        let should_ack = {
            let mut p = pending.borrow_mut();
            if p.input.is_some() && p.input_buffer.is_empty() && !p.input_acked {
                p.input_acked = true;
                true
            } else {
                false
            }
        };
        if should_ack {
            es.borrow().input_sync.ack();
        }

        // 7) 終了判定: 保留中のホスト Promise が無く、かつマイクロタスクも
        //    空なら、モジュールタスク（トップレベル await）が settled した
        //    とみなせる（V8 版で `run_script` の future が完了する条件と同義）。
        let all_empty = {
            let p = pending.borrow();
            p.timers.is_empty() && p.trans.is_empty() && p.audio.is_empty() && p.input.is_none()
        };
        if matches!(pump, PumpState::Idle) && all_empty {
            log::info!("tsint: スクリプトのイベントループが終了しました");
            return Ok(());
        }

        // 8) 次のティックまでの待機時間。次のタイマー/オーディオ猶予期限か、
        //    無ければ IDLE_POLL_MAX 上限でポーリングする（入力の反応性とビジーループ
        //    回避のバランス）。旧実装は 50ms 上限だったが、headless.rs のフレーム
        //    ループ（16ms 刻み）より粗いポーリング間隔だと、`--autotest` が送る
        //    move/click イベントの取りこぼし〜処理までの遅延がフレームより長くなり得て
        //    dist（tsint 専用）ビルドの `--autotest` フレーム差異の主要因の一つだった
        //    （`docs/implementation-notes.md`「2026-07-13 dist フレーム差異の調査と
        //    headless 決定論化」参照）。IDLE_POLL_MAX をフレーム刻みより十分小さくして
        //    この遅延の上限を縮める。
        if !resolved_any {
            // ヘッドレスの tick ドレインバリア: このループの先頭で読んだ `tick_req`
            // （`now` より先に読んでいるので `now` が対応する `Clock::advance()` を
            // 確実に観測している）の時点で処理すべきものは全て処理し切った
            // （timer/trans/audio/input のいずれも今回は何も解決しなかった）ので、
            // headless 側へ完了を報告する。windowed/V8 では誰も待たないので無害。
            es.borrow().tick_sync.mark_complete(tick_req);
            let sleep_for = {
                let p = pending.borrow();
                let now = es.borrow().clock.now();
                let next_timer = p
                    .timers
                    .iter()
                    .map(|(d, _)| d.saturating_duration_since(now))
                    .min();
                // 再生終了・トランジション完了はフラグを見に行く方式なので、待っている間だけ短い間隔で起きる
                // （トランジション完了は通常、完了通知で起こされる）。
                let audio = (!p.audio.is_empty()).then_some(AUDIO_POLL);
                let trans = (!p.trans.is_empty()).then_some(TRANS_POLL);
                [next_timer, audio, trans].into_iter().flatten().min()
            };
            let tick_sync = es.borrow().tick_sync.clone();
            intake.sleep(sleep_for, &tick_sync, tick_req, wake_seen);
        }
    }
}

