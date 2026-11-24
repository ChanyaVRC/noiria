//! OpState に格納する V8 固有の共有状態の型定義。
//!
//! エンジン非依存の状態（シーン・オーディオ送信路・ゲームディレクトリ・
//! テキスト/動画の内部状態等）は `engine_state::EngineState` にまとめてあり、
//! ここには「V8 (deno_core / tokio) の型に依存するため EngineState に同居
//! させられないもの」だけが残る。

use std::cell::Cell;
use std::rc::Rc;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::bridge::InputEvent;

/// 入力イベント受信口。同時に await するのは TS 側の単一ポンプのみという前提。
#[derive(Clone)]
pub(crate) struct InputRx(pub Rc<tokio::sync::Mutex<UnboundedReceiver<InputEvent>>>);

/// ヘッドレスのロックステップ同期用: `op_input_next_event` が呼ばれる（＝TS 側の
/// 入力ポンプが次のイベント待ちに入る）たびに 1 増える世代カウンタ。
/// `run_script` のイベントループラッパが「この世代でまだ ack していない状態で
/// イベントループが Pending（＝マイクロタスクを全て消化して静止）になった」瞬間に
/// `EngineState::input_sync.ack()` を呼ぶ（`docs/implementation-notes.md`
/// 「2026-07-13 headless ロックステップ完全決定論化」参照）。
#[derive(Clone, Default)]
pub(crate) struct InputWaitEpoch(pub Rc<Cell<u64>>);
