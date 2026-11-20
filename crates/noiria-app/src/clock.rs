//! 現在時刻の供給源を1箇所に集約する `Clock` 抽象。
//!
//! `--autotest`（ヘッドレス）実行専用の決定論化のために導入した。windowed 実行
//! (win32/x11) は今まで通り各所で `Instant::now()` を直接呼んでおり触っていない
//! （挙動不変）。ヘッドレスの `Clock::Virtual` はスクリプトスレッド
//! (`engine_state.rs` のアニメーション開始時刻の記録) と headless.rs のフレーム
//! ループ（screenshot/click のスケジューリング、tween/transition の進捗評価）の
//! 両方から共有される 1 個の `Arc<AtomicU64>` を指し、**headless.rs のフレーム
//! ループだけ**がフレームごとに固定刻み（16ms）で進める。実時間の
//! `thread::sleep` のブレやスクリプトスレッドとの相対スケジューリング差から
//! 「現在時刻」の値そのものを切り離すことで、run 間のフレーム差異を減らす
//! （根拠は `docs/implementation-notes.md`「2026-07-13 dist フレーム差異の調査と
//! headless 決定論化」参照）。
//!
//! JS/tsint 側のタイマー（`sys.wait`/`op_wait`、tsint のタイマーヒープ、
//! audio_wait_end の猶予）の期限計算はこの Clock を基準にする（`Clock::Real` では
//! 従来通り実時間、`Clock::Virtual` では仮想 tick に対して決定的）。
//!
//! `nanos` の `Ordering::Relaxed` は `crate::bridge::TickSync` の
//! request/requested_generation（`Ordering::SeqCst`）が運ぶ happens-before に
//! 依存して安全になっている（`TickSync` のドキュメント参照）。ここを弱めるだけの
//! 変更をする場合は `TickSync` 側の正しさの根拠も合わせて見直すこと。
//!
//! `tx`（`tokio::sync::watch::Sender<u64>`）は `op_wait`（V8 版、`ops_sys.rs`）の
//! 仮想分岐が「実時間 1ms ポーリング」ではなく「仮想クロック前進の通知で起きる」ように
//! するための起床トリガ。`advance()` は `nanos` を進めた**直後**に必ず最新値を
//! `send` する（`watch` は最新値だけを保持するので lost-wakeup が無い）。判定は
//! 起こされた後に読む `now()`（＝ `nanos` の値）で行うので、`tx` 自体は「起こす」
//! 役目だけを持ち、時刻の正のソースは引き続き `nanos` 一本のまま
//! （`docs/implementation-notes.md`「2026-07-13 V8 完全決定論化」参照）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;

#[derive(Clone)]
pub enum Clock {
    /// 通常実行（windowed）およびテスト。`Instant::now()` をそのまま返す
    /// （既存挙動と完全に同じ)。
    Real,
    /// ヘッドレス自動テスト専用。`base` から `nanos` だけ経過した時刻を返す。
    /// `nanos` を進めるのは headless.rs のフレームループのみ。
    Virtual {
        base: Instant,
        nanos: Arc<AtomicU64>,
        tx: Arc<watch::Sender<u64>>,
    },
}

impl Clock {
    /// `base` を起点とする仮想クロックを作る（`nanos` = 0 から開始）。
    pub fn new_virtual(base: Instant) -> Self {
        let (tx, _rx) = watch::channel(0u64);
        Clock::Virtual {
            base,
            nanos: Arc::new(AtomicU64::new(0)),
            tx: Arc::new(tx),
        }
    }

    /// 通常実行（windowed/テスト）かどうか。`op_wait`（V8 版）が実時間スリープ
    /// （`Clock::Real`、既存挙動）と仮想時刻の watch 起床待ち（`Clock::Virtual`）を
    /// 選ぶのに使う（`docs/implementation-notes.md`「2026-07-13 headless
    /// ロックステップ完全決定論化」参照）。
    #[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
    pub fn is_real(&self) -> bool {
        matches!(self, Clock::Real)
    }

    pub fn now(&self) -> Instant {
        match self {
            Clock::Real => Instant::now(),
            Clock::Virtual { base, nanos, .. } => {
                *base + Duration::from_nanos(nanos.load(Ordering::Relaxed))
            }
        }
    }

    /// headless.rs のフレームループだけが呼ぶ想定。`Clock::Real` では no-op。
    /// `nanos` を進めた直後に `tx` へ新値を `send` し、`op_wait`（V8 版）の
    /// 仮想分岐で `rx.changed()` を待っているタスクを起こす。
    pub fn advance(&self, step: Duration) {
        if let Clock::Virtual { nanos, tx, .. } = self {
            let new_nanos =
                nanos.fetch_add(step.as_nanos() as u64, Ordering::Relaxed) + step.as_nanos() as u64;
            let _ = tx.send(new_nanos);
        }
    }

    /// `op_wait`（V8 版、`ops_sys.rs`）の仮想分岐が起床トリガとして使う watch
    /// receiver を取得する。`Clock::Real` では誰も `send` しないダミーチャンネルの
    /// receiver を返す（`is_real()` 側の分岐では `sys.wait` が従来通り
    /// `tokio::time::sleep` を使うため、実際にはこの receiver は読まれない）。
    /// `subscribe()` は現在値を既読として始めるため、生成前の `advance()` を新しい
    /// `op_wait` が現在世代の通知として誤消費しない。
    #[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
    pub fn watch_rx(&self) -> watch::Receiver<u64> {
        match self {
            Clock::Real => watch::channel(0u64).1,
            Clock::Virtual { tx, .. } => tx.subscribe(),
        }
    }
}
