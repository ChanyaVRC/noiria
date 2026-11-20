//! システム系 ops: ログ・時間・待機・ファイル・セーブ・スクリーンショット・終了。
//! 実処理は `engine_state::EngineState` に委譲する（V8 非依存のロジック）。
//! `op_wait` の待機だけは V8/tokio 固有のまま残す。

use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{op2, OpState};
use deno_error::JsErrorBox;
use noiria_core::savedata;

use crate::script::engine_state::EngineState;
use crate::script::numconv::{to_u32, wait_duration};
use crate::script::util::err;

#[op2(fast)]
pub fn op_log(state: &mut OpState, #[string] level: &str, #[string] msg: &str) {
    state.borrow::<EngineState>().sys_log(level, msg);
}

/// アプリ起動からのミリ秒。
#[op2(fast)]
pub fn op_now(state: &mut OpState) -> f64 {
    state.borrow::<EngineState>().now()
}

/// `sys.wait(ms)`。windowed（`Clock::Real`）は従来どおり実時間の tokio スリープ。
/// ヘッドレス（`Clock::Virtual`）は仮想時刻に対する期限を、仮想クロック前進の
/// watch 通知（`crate::clock::Clock::watch_rx`）で起こされる形で待つ: 期限は
/// 「仮想 tick 番号」に対して決定的で、かつ headless.rs の `Clock::advance` が
/// 起こすまでは一切実時間で起きないため、`sys.wait` の完了 tick が run ごとに
/// ブレない（KAG プレイヤは waitAdvance のデバウンス等で `sys.wait(40)` 等を多用して
/// おり、旧実装（実時間 1ms ポーリング）はこれが実時間依存でレースして
/// `--autotest` のフレーム差異として可視化されていた。
/// `docs/implementation-notes.md`「2026-07-13 V8 完全決定論化」参照）。
///
/// 待機に入る間だけ `EngineState::pending_waits` を ++/-- する
/// （headless.rs が「V8 が仮想時間待ちで in-flight かどうか」を読むためのカウンタ。
/// `crate::bridge::PendingWaits` 参照）。ガード構造体でスコープ終了時に必ず -- する
/// ことで、future がキャンセル（drop）された場合でもカウンタが取り残されない。
///
/// deno_core は保留中 op の内部 await が進むたびに外側の `poll_event_loop` を
/// 再ポーリングせず、op 自体が完了したときだけ外側を起こす。そのため期限前の tick は
/// この future が直接 `tick_sync.react()` を呼ぶ。期限へ到達した tick だけは反応を
/// 保留し、`mod.rs::run_script` が Promise 継続を含むマイクロタスクを drain した後に
/// flush する。期限到達時にも先に react すると、継続が次の仮想 tick へずれるため。
/// watch receiver は wait ごとに新規購読し、登録前の仮想 tick を未読通知として扱わない。
///
/// **同時に複数の `op_wait` が in-flight のとき**（`sys.wait` を複数箇所が並行に
/// 呼んでいる場合）、直接反応と drain 後の保留反応を合わせて「期待される反応者数ぶん
/// 揃うまで完了報告しない」
/// （`headless.rs` が `request_for(pending_waits)` で反応者数を渡す）。1 個目の
/// `op_wait` だけの反応で「この tick は完了」と誤認すると、まだこの tick に反応して
/// いない他の `op_wait`（の継続処理）がスクリーンショット後にずれ込む可能性がある
/// （`TickSync::react` のドキュメント参照）。
#[op2]
pub async fn op_wait(state: Rc<RefCell<OpState>>, ms: f64) {
    let (clock, pending_waits, tick_sync) = {
        let s = state.borrow();
        let es = s.borrow::<EngineState>();
        (
            es.clock.clone(),
            es.pending_waits.clone(),
            es.tick_sync.clone(),
        )
    };
    let dur = wait_duration(ms);
    if clock.is_real() {
        tokio::time::sleep(dur).await;
    } else {
        let mut rx = clock.watch_rx();
        let deadline = clock.now() + dur;
        if clock.now() < deadline {
            pending_waits.inc();
            struct DecOnDrop(crate::bridge::PendingWaits);
            impl Drop for DecOnDrop {
                fn drop(&mut self) {
                    self.0.dec();
                }
            }
            let _guard = DecOnDrop(pending_waits);
            while clock.now() < deadline {
                let _ = rx.changed().await;
                let gen = tick_sync.requested_generation();
                if clock.now() < deadline {
                    tick_sync.react(gen);
                } else {
                    tick_sync.defer_react(gen);
                }
            }
        }
    }
}

#[op2(fast)]
pub fn op_quit(state: &mut OpState) {
    state.borrow::<EngineState>().quit();
}

#[op2(fast)]
pub fn op_set_title(state: &mut OpState, #[string] title: String) {
    state.borrow::<EngineState>().set_title(title);
}

/// ゲームディレクトリ内のテキストファイル (.ks 等) を読む。
#[op2]
#[string]
pub fn op_read_text(state: &mut OpState, #[string] rel: &str) -> Result<String, JsErrorBox> {
    state.borrow::<EngineState>().read_text(rel).map_err(err)
}

#[op2(fast)]
pub fn op_file_exists(state: &mut OpState, #[string] rel: &str) -> bool {
    state.borrow::<EngineState>().file_exists(rel)
}

#[op2(fast)]
pub fn op_save_write(
    state: &mut OpState,
    #[string] slot: &str,
    #[string] json: &str,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .save_write(slot, json)
        .map_err(err)
}

#[op2]
#[string]
pub fn op_save_read(
    state: &mut OpState,
    #[string] slot: &str,
) -> Result<Option<String>, JsErrorBox> {
    state.borrow::<EngineState>().save_read(slot).map_err(err)
}

#[op2]
#[serde]
pub fn op_save_list(state: &mut OpState) -> Result<Vec<savedata::SlotInfo>, JsErrorBox> {
    state.borrow::<EngineState>().save_list().map_err(err)
}

/// [erasebookmark] の真の削除（W7）。
#[op2(fast)]
pub fn op_save_delete(state: &mut OpState, #[string] slot: &str) -> Result<(), JsErrorBox> {
    state.borrow::<EngineState>().save_delete(slot).map_err(err)
}

/// 現在のシーンを合成してセーブディレクトリへ PNG 保存（サムネイル/オートテスト用）。
#[op2(fast)]
pub fn op_screenshot(state: &mut OpState, #[string] slot: &str) -> Result<(), JsErrorBox> {
    state.borrow::<EngineState>().screenshot(slot).map_err(err)
}

/// 現在のシーンを縮小して保持する（メニューを開く直前の画面をサムネイルにするため）。
/// w/h が 0 なら規定寸法、それ以外はその寸法（モダン UI の大きいカード用）。
#[op2(fast)]
pub fn op_thumb_capture(state: &mut OpState, w: f64, h: f64) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .thumb_capture_sized(to_u32(w), to_u32(h))
        .map_err(err)
}

/// セーブ一覧のサムネイルを別スレッドで先に復号しておく（表示には影響しない）。
#[op2(fast)]
pub fn op_prefetch_save_thumbs(state: &mut OpState, w: f64, h: f64) {
    state
        .borrow::<EngineState>()
        .prefetch_save_thumbs(to_u32(w), to_u32(h));
}

/// ゲーム資産の画像（改行区切りのパス）を w x h の縮小サムネイルにして別スレッドで先に作っておく。
#[op2(fast)]
pub fn op_prefetch_images_scaled(state: &mut OpState, #[string] paths: &str, w: f64, h: f64) {
    let list = paths
        .split('\u{a}')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    state
        .borrow::<EngineState>()
        .prefetch_images_scaled(list, to_u32(w), to_u32(h));
}

/// 直近に保持したサムネイルを `<slot>.png` へ書く。
#[op2(fast)]
pub fn op_thumb_save(state: &mut OpState, #[string] slot: &str) -> Result<(), JsErrorBox> {
    state.borrow::<EngineState>().thumb_save(slot).map_err(err)
}

/// 保存日時の表示用文字列（`YYYY/MM/DD HH:MM`）。
#[op2]
#[string]
pub fn op_wall_clock_string(state: &mut OpState) -> String {
    state.borrow::<EngineState>().wall_clock_string()
}
