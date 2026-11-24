//! テキストレイヤ ops。レイアウト/ラスタはスクリプトスレッド上で行い、
//! Scene にはラスタ済みグリフ (RasterText) を渡す。テキストは常に fore ページ対象。
//! 実処理は `engine_state::EngineState` に委譲する（V8 非依存のロジック）。

use deno_core::{op2, OpState};
use deno_error::JsErrorBox;
use noiria_core::text::GlyphRect;

use crate::script::engine_state::{EngineState, PrintResult, TextSegmentArgs, TextStyleArgs};
use crate::script::numconv::to_u32;
use crate::script::util::err;

#[op2]
pub(crate) fn op_text_style(
    state: &mut OpState,
    id: f64,
    #[serde] style: TextStyleArgs,
) -> Result<(), JsErrorBox> {
    let style = style.into_style();
    if !style.line_height.is_finite() {
        return Err(JsErrorBox::type_error("lineHeight must be a finite f32"));
    }
    state
        .borrow_mut::<EngineState>()
        .text_style(to_u32(id), style);
    Ok(())
}

#[op2]
#[serde]
pub(crate) fn op_text_print(
    state: &mut OpState,
    id: f64,
    #[serde] segments: Vec<TextSegmentArgs>,
    append: bool,
    cps: f64,
) -> Result<PrintResult, JsErrorBox> {
    state
        .borrow_mut::<EngineState>()
        .text_print(
            to_u32(id),
            segments
                .into_iter()
                .map(TextSegmentArgs::into_segment)
                .collect(),
            append,
            cps,
        )
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_text_reflow(state: &mut OpState, id: f64) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<EngineState>()
        .text_reflow(to_u32(id))
        .map_err(err)
}

/// リベールを即時完了させる。
#[op2(fast)]
pub(crate) fn op_text_complete(state: &mut OpState, id: f64) {
    state.borrow::<EngineState>().text_complete(to_u32(id));
}

/// リベールが完了しているか。
#[op2(fast)]
pub(crate) fn op_text_is_done(state: &mut OpState, id: f64) -> bool {
    state.borrow::<EngineState>().text_is_done(to_u32(id))
}

#[op2(fast)]
pub(crate) fn op_text_clear(state: &mut OpState, id: f64) {
    state.borrow_mut::<EngineState>().text_clear(to_u32(id));
}

/// グリフのヒット矩形（リンク選択肢・メニュー行判定用）。
#[op2]
#[serde]
pub(crate) fn op_text_rects(state: &mut OpState, id: f64) -> Vec<GlyphRect> {
    state.borrow::<EngineState>().text_rects(to_u32(id))
}
