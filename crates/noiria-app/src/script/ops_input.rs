//! 入力 ops。TS 側の単一ポンプが op_input_next_event を呼び続け、
//! waitClick 等の高級 API は TS 側で構築する。

use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{op2, OpState};

use crate::bridge::InputEvent;
use crate::script::state::{InputRx, InputWaitEpoch};

/// 次の入力イベントを待つ。チャンネルが閉じたら kind="quit" を返す。
///
/// ヘッドレス専用のロックステップ同期（`docs/implementation-notes.md`
/// 「2026-07-13 headless ロックステップ完全決定論化」参照）: このオペが呼ばれた時点
/// （＝TS 側の入力ポンプ `runtime/api.ts::InputHub.pump` が次のイベント待ちに入った
/// 時点）で `InputWaitEpoch` を 1 進める。ack 自体はここでは出さない — ポンプは
/// イベントをハンドラへ配送した直後にこのオペを再度呼ぶが、KAG プレイヤの本処理
/// （クリックによるシナリオ進行等）はその後のマイクロタスクとして走るため、ここで
/// ack すると早すぎる。実際の ack は `run_script` のイベントループラッパが
/// 「マイクロタスクを全て消化して Pending になった」瞬間に出す（`state.rs` の
/// `InputWaitEpoch` のドキュメント参照）。
#[op2]
#[serde]
pub async fn op_input_next_event(state: Rc<RefCell<OpState>>) -> InputEvent {
    let (rx, epoch) = {
        let s = state.borrow();
        (
            s.borrow::<InputRx>().clone(),
            s.borrow::<InputWaitEpoch>().clone(),
        )
    };
    epoch.0.set(epoch.0.get() + 1);
    let mut rx = rx.0.lock().await;
    match rx.recv().await {
        Some(ev) => ev,
        None => InputEvent {
            kind: "quit",
            x: 0.0,
            y: 0.0,
            button: None,
            key: None,
            delta: None,
            t: 0.0,
        },
    }
}
