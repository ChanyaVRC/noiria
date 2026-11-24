//! トランジション ops。実処理は `engine_state::EngineState` に委譲する
//! （V8 非依存のロジック）。非同期の完了待ちだけは V8/tokio 固有のまま残す。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use deno_core::{op2, OpState};
use deno_error::JsErrorBox;

use crate::script::engine_state::{EngineState, TransArgs};
use crate::script::util::err;

#[op2]
pub(crate) fn op_trans_begin(
    state: &mut OpState,
    #[serde] args: TransArgs,
) -> Result<(), JsErrorBox> {
    state.borrow::<EngineState>().trans_begin(args).map_err(err)
}

#[op2(fast)]
pub(crate) fn op_trans_skip(state: &mut OpState) {
    state.borrow::<EngineState>().trans_skip();
}

/// トランジション完了（ページスワップ）まで待つ。
#[op2]
pub(crate) async fn op_trans_wait(state: Rc<RefCell<OpState>>) {
    let notify = {
        let state = state.borrow();
        state.borrow::<EngineState>().trans_notify.clone()
    };
    loop {
        {
            let state = state.borrow();
            if !state.borrow::<EngineState>().trans_active() {
                return;
            }
        }
        // notified() 登録前の完了を取りこぼさないよう 50ms のポーリングを安全網にする
        let _ = tokio::time::timeout(Duration::from_millis(50), notify.notified()).await;
    }
}
