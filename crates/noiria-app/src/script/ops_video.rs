//! 動画再生 ops。実処理は `engine_state::EngineState` に委譲する
//! （V8 非依存のロジック。再生スレッド本体も `engine_state::playback_loop` へ移設済み）。

use deno_core::{op2, OpState};
use deno_error::JsErrorBox;

use crate::script::engine_state::{video_optional_frame, EngineState};
use crate::script::numconv::to_u32;
use crate::script::util::err;

#[op2(fast)]
pub(crate) fn op_video_play(
    state: &mut OpState,
    id: f64,
    #[string] path: &str,
    looping: bool,
) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<EngineState>()
         .video_play(to_u32(id), path, looping)
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_video_play_at(
    state: &mut OpState,
    id: f64,
    #[string] path: &str,
    looping: bool,
    frame: f64,
    position_ms: f64,
) -> Result<(), JsErrorBox> {
    let frame = video_optional_frame(frame).map_err(err)?;
    state
        .borrow_mut::<EngineState>()
        .video_play_at(
            to_u32(id),
            path,
            looping,
            frame,
            (position_ms != -1.0).then_some(position_ms),
        )
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_video_stop(state: &mut OpState) {
    state.borrow_mut::<EngineState>().video_stop();
}

#[op2(fast)]
pub(crate) fn op_video_is_playing(state: &mut OpState) -> bool {
    state.borrow::<EngineState>().video_is_playing()
}

// ---- W6: openvideo/preparevideo/playvideo/pausevideo/resumevideo/rewindvideo ----

#[op2(fast)]
pub(crate) fn op_video_open(
    state: &mut OpState,
    id: f64,
    #[string] path: &str,
    looping: bool,
) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<EngineState>()
         .video_open(to_u32(id), path, looping)
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_video_prepare(
    state: &mut OpState,
    id: f64,
    #[string] path: &str,
    looping: bool,
) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<EngineState>()
         .video_prepare(to_u32(id), path, looping)
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_video_prepare_opened(state: &mut OpState, id: f64) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<EngineState>()
         .video_prepare_opened(to_u32(id))
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_video_play_prepared(state: &mut OpState, id: f64) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<EngineState>()
         .video_play_prepared(to_u32(id))
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_video_pause(state: &mut OpState) {
    state.borrow_mut::<EngineState>().video_pause();
}

#[op2(fast)]
pub(crate) fn op_video_resume(state: &mut OpState) {
    state.borrow_mut::<EngineState>().video_resume();
}

#[op2(fast)]
pub(crate) fn op_video_rewind(state: &mut OpState) {
    state.borrow_mut::<EngineState>().video_rewind();
}

#[op2(fast)]
pub(crate) fn op_video_is_paused(state: &mut OpState) -> bool {
    state.borrow::<EngineState>().video_is_paused()
}

#[op2(fast)]
pub(crate) fn op_video_seek_frame(state: &mut OpState, target: f64) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<EngineState>()
        .video_seek_frame(to_u32(target) as usize)
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_video_seek_position(state: &mut OpState, position_ms: f64) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<EngineState>()
        .video_seek_position(position_ms)
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_video_set_period_event(state: &mut OpState, frame: f64) {
    state
        .borrow_mut::<EngineState>()
        .video_set_period_event(to_u32(frame) as usize);
}

#[op2(fast)]
pub(crate) fn op_video_cancel_period_event(state: &mut OpState) {
    state
        .borrow_mut::<EngineState>()
        .video_cancel_period_event();
}

#[op2(fast)]
pub(crate) fn op_video_set_segment_loop(
    state: &mut OpState,
    start: f64,
    end: f64,
) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<EngineState>()
        .video_set_segment_loop(to_u32(start) as usize, to_u32(end) as usize)
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_video_cancel_segment_loop(state: &mut OpState) {
    state
        .borrow_mut::<EngineState>()
        .video_cancel_segment_loop();
}

#[op2(fast)]
pub(crate) fn op_video_take_event(state: &mut OpState, #[string] kind: &str) -> bool {
    state.borrow_mut::<EngineState>().video_take_event(kind)
}

#[op2(fast)]
pub(crate) fn op_video_generation(state: &mut OpState) -> f64 {
    state.borrow::<EngineState>().video_generation()
}
