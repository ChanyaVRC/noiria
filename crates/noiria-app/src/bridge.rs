//! スレッド間ブリッジ: メイン(Win32) ⇔ スクリプト(V8) ⇔ オーディオ。

#[cfg(windows)]
use std::sync::atomic::AtomicIsize;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use noiria_core::scene::Scene;
use serde::Serialize;

pub type SharedScene = Arc<Mutex<Scene>>;

/// シーンのロック。別スレッドがロックを握ったまま panic して毒化していても中身を取り出す。描画ループが先に
/// `PoisonError` で落ちると、本来のエラー（毒化の原因になった panic）が報告されないまま窓だけが消えるため。
pub fn lock_scene(scene: &SharedScene) -> std::sync::MutexGuard<'_, Scene> {
    scene.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `lock().unwrap()` の毒化の連鎖を断つ `Mutex` 拡張。別スレッドがロックを握ったまま panic しても
/// 中身を取り出す（`lock_scene` と同じ理由。次に触るスレッドが `PoisonError` で落ちると本当のエラーが埋もれる）。
pub trait LockExt<T> {
    fn lock_unpoisoned(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockExt<T> for std::sync::Mutex<T> {
    fn lock_unpoisoned(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 描画ループが次に起きるまでの待ち時間。アニメ中は「前回の present の開始時刻から `interval`」を次の期限にする
/// （present 後に毎回 `interval` 待つと、周期が `interval` + 合成・転送の時間になり 60fps を割る）。
/// 動いていないときは、描き直しが間引かれていればその期限（`frame_due_in`）、閉じる猶予があればその残りまで。
pub fn frame_wait(
    animating: bool,
    interval: std::time::Duration,
    since_present: std::time::Duration,
    frame_due_in: Option<std::time::Duration>,
    close_remaining: Option<std::time::Duration>,
    idle: std::time::Duration,
) -> std::time::Duration {
    let mut wait = if animating {
        interval.saturating_sub(since_present)
    } else {
        idle
    };
    if let Some(d) = frame_due_in {
        wait = wait.min(d);
    }
    if let Some(rem) = close_remaining {
        wait = wait.min(rem);
    }
    wait
}

/// 描画ループがこの周期に再合成すべきか。動いている間（`animating`）と、動きが終わった直後の 1 周期
/// （`was_animating`）は描く。終わった周期は `animating` が false で `revision` も変わらないため、これが
/// 無いと文字送りの最後のグリフ（「…」の 」 など）が次の更新まで画面に出ない。
pub fn needs_redraw(revision: u64, last_drawn: u64, animating: bool, was_animating: bool) -> bool {
    revision != last_drawn || animating || was_animating
}

/// メインループへ送るユーザーイベント。
#[derive(Debug)]
pub enum UserEvent {
    /// シーンが変更された。再描画を要求する。
    Redraw,
    SetTitle(String),
    Quit,
}

/// メインループの起こし先。通常起動では Win32 ウィンドウへ
/// `PostMessageW(WM_APP)` で起こす（`hwnd` はウィンドウ生成前は 0 で、
/// その間はチャンネル送信のみで良い。次のポンプで拾われる）。
/// ヘッドレス (--autotest) ではチャンネルのみ。
#[derive(Clone)]
pub enum WakeSink {
    #[cfg(windows)]
    Win32 {
        tx: crossbeam_channel::Sender<UserEvent>,
        hwnd: Arc<AtomicIsize>,
        /// 起こしのメッセージが投函済みで、窓がまだ取り込んでいないか。窓ごとに持つ（プロセス全体で 1 つだと、
        /// 並列の 2 窓が互いの起こしを抑止し得る）。
        posted: Arc<std::sync::atomic::AtomicBool>,
    },
    Channel(crossbeam_channel::Sender<UserEvent>),
}

impl WakeSink {
    pub fn send(&self, ev: UserEvent) {
        match self {
            #[cfg(windows)]
            WakeSink::Win32 { tx, hwnd, posted } => {
                let _ = tx.send(ev);
                crate::win32::post_wake(hwnd.load(Ordering::SeqCst), posted);
            }
            WakeSink::Channel(c) => {
                let _ = c.send(ev);
            }
        }
    }
}

/// スクリプトへ渡す入力イベント。JS へは JSON として渡る。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputEvent {
    /// "click" | "move" | "wheel" | "key" | "keyup"（keyup は Control/Shift/Enter/Space のみ）
    pub kind: &'static str,
    /// ゲーム座標系 (スケール補正済み)
    pub x: f64,
    pub y: f64,
    /// click: "left" | "right" | "middle"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<&'static str>,
    /// key/keyup: "Enter" | "Escape" | "Control" | "Shift" | " " | 文字。閉じる操作は key:"WindowClose"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// wheel: 上が正
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
    /// アプリ起動からのミリ秒
    pub t: f64,
}

/// ヘッドレス（`--autotest`）専用: headless.rs のフレームループとスクリプトスレッドの
/// 入力処理を明示的な同期点で結ぶロックステップ用カウンタ。
///
/// スクリプトスレッド側は「入力ポンプ（`runtime/api.ts::InputHub.pump`）が直前の
/// イベントの同期処理を終えて次のイベント待ちへ戻った」瞬間に [`ack`] を呼ぶ
/// （V8 は `ops_input.rs::op_input_next_event`、tsint は `tsint_engine.rs` の
/// `op_input_next_event` ネイティブ実装。どちらも呼び出しの最初の一手として ack する）。
/// headless.rs は入力イベントを送った直後にこの値の変化を待ってから仮想クロックを
/// 進めることで、「スクリプト側がいつ `Clock::now()` を読むか」という cross-thread
/// レースを閉じる（`docs/implementation-notes.md`「2026-07-13 headless
/// ロックステップ完全決定論化」参照）。windowed 経路は生成するだけで誰も観測しない
/// ため挙動に影響しない。
#[derive(Clone, Default)]
pub struct InputSync(Arc<AtomicU64>);

impl InputSync {
    pub fn new() -> Self {
        Self(Arc::new(AtomicU64::new(0)))
    }

    /// スクリプトスレッド側: 「次の入力待ちへ戻った」ことを記録する。
    pub fn ack(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    /// headless.rs 側: 現在の世代値を読む。
    pub fn generation(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// ヘッドレス: `Clock::advance()` によるフレーム刻みの前進と、スクリプト側の
/// タイマー/オーディオ猶予の期限処理との間に残るレースを閉じる「tick ドレイン
/// バリア」。
///
/// [`InputSync`] が「1 個の入力イベントの同期処理完了」を伝えるのに対し、
/// `TickSync` は「headless が進めた仮想 tick について、スクリプト側がその時点で
/// 処理すべきタイマー/オーディオ猶予/入力を全て処理し切ってアイドルへ戻った」ことを
/// 伝える、より一般的なバリア。**tsint** の `drive()` ループ（実時間ポーリングで
/// 期限をチェックする busy-loop）と、**V8** の `run_script` ポーリングループ
/// （`op_wait` の watch バリアと組み合わせて使う、`crate::bridge::PendingWaits`
/// 参照）の両方が完了を報告する（`docs/implementation-notes.md`
/// 「2026-07-13 tsint tick ドレインバリア」「2026-07-13 V8 完全決定論化」参照）。
/// windowed 経路は（`Clock::Real` なので `advance` 自体が no-op、`request` が
/// 呼ばれても誰も待たないので無害）挙動に影響しない。
///
/// **正しさの根拠（happens-before、tsint）**: [`request`]/[`requested_generation`]
/// は共に `Ordering::SeqCst`（release/acquire を含む）なので、tsint 側が
/// `requested_generation()` である値以上を観測したとき、headless がその値を返した
/// `request()` 呼び出しの**前**に行った `Clock::advance()`（`clock.rs` の `nanos` は
/// `Ordering::Relaxed`）も、`requested_generation()` の**直後**に読む `Clock::now()`
/// から確実に見える（release シーケンスによる推移的な happens-before。`nanos` 自体を
/// `Relaxed` のままにできるのはこのため）。**tsint の drive ループは必ず
/// `requested_generation()` → `Clock::now()` の順で読むこと**（逆順にすると
/// この保証が崩れる）。tsint は busy-loop（毎回タイムアウト無しで再実行される）なので
/// headless 側の呼び出し順は `advance()` → `request()`。
///
/// **正しさの根拠（happens-before、V8）**: V8 は tsint と異なり wake 駆動
/// （`poll_event_loop` は起こされたときしか再ポーリングされない）ため、headless 側の
/// 呼び出し順は tsint と**逆**: `request()` を `Clock::advance()`（の内部で watch へ
/// `send` する）より**前**に呼ぶ（`headless.rs` 参照）。こうすることで、
/// `Clock::advance` の watch 通知で V8 の `op_wait` が起こされ、そのままイベント
/// ループが静止して `requested_generation()` を読む時点で、必ずこの `request()` を
/// 観測できる（`request()` → `advance()` の内部の `send()` → watch の同期による
/// wake 配送 → `requested_generation()` の読み取り、という単一スレッド内 program
/// order + channel 同期の推移的合成）。V8 はこのバリアを `PendingWaits` が 0
/// より大きい（in-flight な仮想 `op_wait` がある）ときだけ使う（0 のとき無条件に
/// 張ると、誰も起こしてくれないアイドル tick で毎回 `ACK_TIMEOUT` に到達する）。
#[derive(Clone, Default)]
pub struct TickSync {
    requested: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    v8: Arc<Mutex<V8TickState>>,
    deferred_count: Arc<AtomicI64>,
}

#[derive(Default)]
struct V8TickState {
    generation: u64,
    remaining: i64,
    deferred: i64,
}

impl TickSync {
    pub fn new() -> Self {
        Self {
            requested: Arc::new(AtomicU64::new(0)),
            completed: Arc::new(AtomicU64::new(0)),
            v8: Arc::new(Mutex::new(V8TickState::default())),
            deferred_count: Arc::new(AtomicI64::new(0)),
        }
    }

    /// headless 側（tsint）: 新しい要求世代を返す（呼び出し側はこの値を
    /// [`wait`][crate::headless] 側の目標値として使う）。`Clock::advance()` の
    /// **後**に呼ぶ（型のドキュメント参照。呼び出し順は `headless.rs` を参照し、
    /// 変更しないこと）。V8 は [`request_for`][Self::request_for] を使う。
    pub fn request(&self) -> u64 {
        self.requested.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// headless 側（V8 専用）: `Clock::advance()` の**前**に呼ぶ。`expected_reactors`
    /// （`PendingWaits::get()` のスナップショット、`advance()` より前に読む）は
    /// 「この advance に反応するはずの `op_wait` の数」。これから `mark_complete`
    /// されるには、この人数ぶんの [`react`][Self::react] 呼び出しが揃う必要がある
    /// （`Clock::advance` の直後に複数の `op_wait` が同時に起きる場合、最初の 1 個の
    /// 反応だけで「この tick は完了」と誤認しないため。`op_wait` が単一の
    /// `poll_event_loop` の Pending 検出だけでは正しく検出できないことは実測で確認
    /// 済み。`docs/implementation-notes.md`「2026-07-13 V8 完全決定論化」参照）。
    pub fn request_for(&self, expected_reactors: i64) -> u64 {
        let mut state = self.v8.lock().unwrap();
        let generation = self.requested.fetch_add(1, Ordering::SeqCst) + 1;
        state.generation = generation;
        state.remaining = expected_reactors.max(1);
        // タイムアウトで次世代へ進んだ場合、旧世代の保留反応を新しい残数へ適用しない。
        state.deferred = 0;
        self.deferred_count.store(0, Ordering::SeqCst);
        generation
    }

    /// スクリプトスレッド側: 最新の要求世代を読む。tsint はこの直後に `Clock::now()`
    /// を読むことで対応する `Clock::advance()` を確実に観測する（型のドキュメント
    /// 参照）。
    pub fn requested_generation(&self) -> u64 {
        self.requested.load(Ordering::SeqCst)
    }

    /// スクリプトスレッド側（tsint の `drive()` のみ）: 指定した要求世代までの
    /// 処理を完了したと報告する（逆行しないよう最大値を保持する）。V8 は
    /// [`react`][Self::react] を使う（`remaining` を経由せずここを直接呼ぶと
    /// 「全 `op_wait` が反応し終わるまで待つ」保証が壊れるため、V8 からは
    /// 呼ばないこと）。
    pub fn mark_complete(&self, gen: u64) {
        self.completed.fetch_max(gen, Ordering::SeqCst);
    }

    /// スクリプトスレッド側（V8 の `op_wait` のみ）: 自分がこの世代の仮想クロック
    /// 前進に反応したことを報告する。`request_for` が設定した `remaining` を
    /// 1 減らし、0 以下になった（＝期待していた全 `op_wait` が反応し終えた）
    /// 呼び出しだけが実際に `mark_complete` する。`ops_sys.rs`（engine-v8）からしか
    /// 呼ばれないため、engine-v8 未使用ビルドでは未使用になる（dead_code を
    /// 明示的に許可する）。
    #[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
    pub fn react(&self, gen: u64) {
        let complete = {
            let mut state = self.v8.lock().unwrap();
            if state.generation != gen {
                return;
            }
            state.remaining -= 1;
            state.remaining <= 0
        };
        if complete {
            self.mark_complete(gen);
        }
    }

    /// V8 の `op_wait` が期限へ到達した tick では、native future の完了だけで
    /// barrier を開けると、その Promise 継続が次の仮想 tick へずれる。外側の
    /// `poll_event_loop` がマイクロタスクを drain するまで反応を保留する。
    #[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
    pub fn defer_react(&self, gen: u64) {
        let mut state = self.v8.lock().unwrap();
        if state.generation != gen {
            return;
        }
        state.deferred += 1;
        self.deferred_count.store(state.deferred, Ordering::SeqCst);
    }

    #[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
    pub fn has_deferred_reactors(&self) -> bool {
        self.deferred_count.load(Ordering::SeqCst) > 0
    }

    /// V8 のイベントループが静止した後、期限到達 waiter の保留反応をまとめて適用する。
    #[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
    pub fn flush_deferred(&self) {
        let complete_generation = {
            let mut state = self.v8.lock().unwrap();
            let count = std::mem::take(&mut state.deferred);
            self.deferred_count.store(0, Ordering::SeqCst);
            state.remaining -= count;
            (count > 0 && state.remaining <= 0).then_some(state.generation)
        };
        if let Some(generation) = complete_generation {
            self.mark_complete(generation);
        }
    }

    /// headless 側: 完了世代を読む。
    pub fn completed_generation(&self) -> u64 {
        self.completed.load(Ordering::SeqCst)
    }
}

/// ヘッドレス（V8 専用）: 仮想 `op_wait`（`ops_sys.rs`）の in-flight 数。
/// `op_wait` の仮想分岐が待機に入る直前に [`inc`][Self::inc]、抜けた直後（正常終了/
/// キャンセルいずれも）に [`dec`][Self::dec] する。headless.rs はフレームループ末尾で
/// これが 0 なら V8 の tick ドレインバリア（`TickSync` 参照）を丸ごとスキップする
/// （アイドル tick、つまり入力待ちだけで in-flight な `op_wait` が無い状態で
/// 無条件にバリアを張ると、誰も起こしてくれないまま `ACK_TIMEOUT`（3秒）に毎 tick
/// 到達してしまうため）。
///
/// tsint はこのカウンタに一切触れない（`tsint_engine.rs` は自前のタイマーヒープ
/// `PendingOps::timers` で `op_wait` を待つため）。よって tsint 実行時は常に 0 のまま
/// —— 既存の tsint 用バリア（`TickSync` + `HeadlessParams::tick_barrier`）の挙動には
/// 影響しない（`docs/implementation-notes.md`「2026-07-13 V8 完全決定論化」参照）。
#[derive(Clone, Default)]
pub struct PendingWaits(Arc<AtomicI64>);

impl PendingWaits {
    pub fn new() -> Self {
        Self(Arc::new(AtomicI64::new(0)))
    }

    /// `op_wait`（V8 版）の仮想分岐: 待機開始時に呼ぶ。`ops_sys.rs`（engine-v8）から
    /// しか呼ばれないため、engine-v8 未使用ビルドでは未使用になる（dead_code を
    /// 明示的に許可する）。
    #[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
    pub fn inc(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    /// `op_wait`（V8 版）の仮想分岐: 待機終了時（正常/キャンセルいずれも）に呼ぶ。
    /// `inc` と同じ理由で dead_code を明示的に許可する。
    #[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
    pub fn dec(&self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }

    /// headless 側: 現在の in-flight 数を読む。
    pub fn get(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod redraw_tests {
    use super::needs_redraw;
    use noiria_core::scene::TextReveal;
    use std::time::{Duration, Instant};

    /// 16ms 周期の描画ループを模して、文字送りが終わった時点の全グリフが最後の描画に含まれることを確かめる。
    fn last_drawn_glyphs(redraw_after_finish: bool) -> usize {
        let t0 = Instant::now();
        let total = 40;
        let reveal = TextReveal { from_glyph: 0, start: Some(t0), cps: 35.0 };
        let (mut last_rev, mut was) = (0u64, false);
        let mut last_shown = 0;
        for k in 0..200u64 {
            let now = t0 + Duration::from_millis(7 + k * 16);
            let animating = !reveal.is_done(total, now);
            let draw = if redraw_after_finish {
                needs_redraw(1, last_rev, animating, was)
            } else {
                1 != last_rev || animating
            };
            if draw {
                last_rev = 1;
                last_shown = reveal.visible_glyphs(total, now);
            }
            was = animating;
        }
        last_shown
    }

    #[test]
    fn a_redraw_follows_the_end_of_motion_so_the_last_glyph_appears() {
        assert_eq!(last_drawn_glyphs(true), 40);
        // 以前の条件では最後の描画が文字送りの途中のまま止まる（これが 」 だけ出ない不具合）
        assert!(last_drawn_glyphs(false) < 40);
    }
}

#[cfg(test)]
mod tests {
    use super::TickSync;

    #[test]
    fn deferred_reactor_completes_only_after_the_event_loop_flushes_it() {
        let sync = TickSync::new();
        let generation = sync.request_for(2);

        sync.react(generation);
        sync.defer_react(generation);
        assert!(sync.completed_generation() < generation);

        sync.flush_deferred();
        assert_eq!(sync.completed_generation(), generation);
    }

    #[test]
    fn stale_reactors_cannot_complete_a_new_generation() {
        let sync = TickSync::new();
        let stale = sync.request_for(1);
        sync.defer_react(stale);

        let current = sync.request_for(2);
        sync.react(stale);
        sync.defer_react(stale);
        sync.flush_deferred();
        assert!(sync.completed_generation() < current);

        sync.react(current);
        assert!(sync.completed_generation() < current);
        sync.react(current);
        assert_eq!(sync.completed_generation(), current);
    }
}

#[cfg(test)]
mod frame_wait_tests {
    use super::frame_wait;
    use std::time::Duration;

    const IV: Duration = Duration::from_millis(16);
    const IDLE: Duration = Duration::from_millis(200);
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// 指摘 4: アニメ中の待ちは「前回の present の開始から `interval`」まで。作業 6ms なら残り 10ms
    /// （旧実装は present の後に毎回 `interval` 待ち、周期が `interval` + 作業時間 = 約 43fps になっていた）。
    #[test]
    fn animating_waits_only_for_the_rest_of_the_interval() {
        assert_eq!(frame_wait(true, IV, ms(6), None, None, IDLE), ms(10));
        assert_eq!(frame_wait(true, IV, ms(0), None, None, IDLE), IV);
        assert_eq!(frame_wait(true, IV, ms(16), None, None, IDLE), Duration::ZERO);
        assert_eq!(frame_wait(true, IV, ms(40), None, None, IDLE), Duration::ZERO, "遅れたら待たずに次へ");
    }

    #[test]
    fn periodic_cadence_is_not_stretched_by_work_time() {
        let work = ms(6);
        let wait = frame_wait(true, IV, work, None, None, IDLE);
        assert_eq!(work + wait, IV);
    }

    #[test]
    fn idle_and_deadlines_take_the_minimum() {
        assert_eq!(frame_wait(false, IV, ms(3), None, None, IDLE), IDLE);
        assert_eq!(frame_wait(false, IV, ms(3), Some(ms(5)), None, IDLE), ms(5));
        assert_eq!(frame_wait(true, IV, ms(3), Some(ms(5)), Some(ms(2)), IDLE), ms(2));
        assert_eq!(frame_wait(false, IV, ms(3), None, Some(ms(7)), Duration::MAX), ms(7));
    }
}
