//! オーディオ ops。実処理はオーディオスレッド (noiria-core::audio) が行う。
//! 同期部分は `engine_state::EngineState` に委譲する（V8 非依存のロジック）。
//! 非同期の再生終了待ちだけは V8/tokio 固有のまま残す（ポーリング）。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use deno_core::{op2, OpState};
use deno_error::JsErrorBox;

use crate::script::engine_state::EngineState;
use crate::script::numconv::to_u32;
use crate::script::util::err;

#[op2(fast)]
pub(crate) fn op_audio_play(
    state: &mut OpState,
    #[string] ch: &str,
    #[string] path: &str,
    looping: bool,
    volume: f64,
    fade_in_ms: f64,
    start_position_ms: f64,
    start_paused: bool,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .audio_play(
            ch,
            path,
            looping,
            volume,
            to_u32(fade_in_ms),
            start_position_ms,
            start_paused,
        )
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_audio_stop(
    state: &mut OpState,
    #[string] ch: &str,
    fade_out_ms: f64,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .audio_stop(ch, to_u32(fade_out_ms))
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_audio_volume(
    state: &mut OpState,
    #[string] ch: &str,
    volume: f64,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .audio_volume(ch, volume)
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_audio_is_playing(state: &mut OpState, #[string] ch: &str) -> Result<bool, JsErrorBox> {
    state
        .borrow::<EngineState>()
        .audio_is_playing(ch)
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_audio_position(state: &mut OpState, #[string] ch: &str) -> Result<f64, JsErrorBox> {
    state
        .borrow::<EngineState>()
        .audio_position(ch)
        .map(|position| position as f64)
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_audio_pause(state: &mut OpState, #[string] ch: &str) -> Result<(), JsErrorBox> {
    state.borrow::<EngineState>().audio_pause(ch).map_err(err)
}

#[op2(fast)]
pub(crate) fn op_audio_resume(state: &mut OpState, #[string] ch: &str) -> Result<(), JsErrorBox> {
    state.borrow::<EngineState>().audio_resume(ch).map_err(err)
}

#[op2(fast)]
pub(crate) fn op_audio_fade_to(
    state: &mut OpState,
    #[string] ch: &str,
    volume: f64,
    ms: f64,
    #[string] then: &str,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .audio_fade_to(ch, volume, to_u32(ms), then)
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_audio_is_fading(state: &mut OpState, #[string] ch: &str) -> Result<bool, JsErrorBox> {
    state
        .borrow::<EngineState>()
        .audio_is_fading(ch)
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_audio_is_finished(state: &mut OpState, #[string] ch: &str) -> Result<bool, JsErrorBox> {
    state
        .borrow::<EngineState>()
        .audio_is_finished(ch)
        .map_err(err)
}

/// チャンネルの再生終了まで待つ（ポーリング）。
#[op2]
pub(crate) async fn op_audio_wait_end(
    state: Rc<RefCell<OpState>>,
    #[string] ch: String,
) -> Result<(), JsErrorBox> {
    let (shared, ch) = {
        let state = state.borrow();
        let es = state.borrow::<EngineState>();
        let ch = noiria_core::audio::channel_index(&ch)
            .ok_or_else(|| err(format!("不明なチャンネル: {ch}")))?;
        (es.audio_shared.clone(), ch)
    };
    // 再生中か、Play 要求済みでデコード中の間は待つ（固定の猶予時間は使わない）。
    while shared.is_playing(ch) || shared.is_play_pending(ch) {
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    Ok(())
}
