//! レイヤ操作 ops。レイヤは fore/back の両ページに対で存在する。
//! 実処理は `engine_state::EngineState` に委譲する（V8 非依存のロジック）。

use deno_core::{op2, OpState};
use deno_error::JsErrorBox;

use crate::script::engine_state::{BlitClip, EngineState, ImageLoadOpts, LayerProps, MoveArgs};
use crate::script::numconv::{to_i32, to_time_ms, to_u32, to_u8};
use crate::script::util::err;

#[op2(fast)]
pub(crate) fn op_layer_new(state: &mut OpState, #[string] kind: &str) -> Result<u32, JsErrorBox> {
    state.borrow::<EngineState>().layer_new(kind).map_err(err)
}

#[op2(fast)]
pub(crate) fn op_layer_free(state: &mut OpState, id: f64) {
    state.borrow::<EngineState>().layer_free(to_u32(id));
}

#[op2]
#[serde]
pub(crate) fn op_layer_load_image(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
    #[string] path: String,
) -> Result<(u32, u32), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_load_image(to_u32(id), page, path)
        .map_err(err)
}

/// セーブ領域のサムネイル（`<slot>.png`）をレイヤへ読み込む。`GameDir` 経由ではない専用経路。
/// w/h が 0 以外ならその寸法へ拡縮する（0 は規定寸法に収める従来動作）。
#[op2]
#[serde]
pub(crate) fn op_layer_load_save_thumb(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
    #[string] slot: &str,
    w: f64,
    h: f64,
) -> Result<(u32, u32), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_load_save_thumb_sized(to_u32(id), page, slot, to_u32(w), to_u32(h))
        .map_err(err)
}

/// 画像を読み、面積平均で w x h へ拡縮してレイヤへ設定する（ギャラリーのサムネイル）。
#[op2]
#[serde]
pub(crate) fn op_layer_load_image_scaled(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
    #[string] path: String,
    w: f64,
    h: f64,
) -> Result<(u32, u32), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_load_image_scaled(to_u32(id), page, path, to_u32(w), to_u32(h))
        .map_err(err)
}

/// グラデーション・角丸・枠線・影つきパネルをレイヤの絵にする（モダン UI）。`spec` は
/// `PanelSpec::from_slice` の数値列。
#[op2]
pub(crate) fn op_layer_panel(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
    #[serde] spec: Vec<f64>,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_panel(to_u32(id), page, &spec)
        .map_err(err)
}

/// KAG の [image]（W5 拡張）: clip=/flipud=/fliplr=/grayscale= を適用してから読み込む。
#[op2]
#[serde]
pub(crate) fn op_layer_load_image_ex(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
    #[string] path: String,
    #[serde] opts: ImageLoadOpts,
) -> Result<(u32, u32), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_load_image_ex(to_u32(id), page, path, opts)
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_layer_clear_image(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_clear_image(to_u32(id), page)
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_layer_fill(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
    w: f64,
    h: f64,
    r: f64,
    g: f64,
    b: f64,
    a: f64,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_fill(
            to_u32(id),
            page,
            to_u32(w),
            to_u32(h),
            to_u8(r),
            to_u8(g),
            to_u8(b),
            to_u8(a),
        )
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_layer_msgbox(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
    w: f64,
    h: f64,
    radius: f64,
    r: f64,
    g: f64,
    b: f64,
    a: f64,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_msgbox(
            to_u32(id),
            page,
            to_u32(w),
            to_u32(h),
            to_u32(radius),
            to_u8(r),
            to_u8(g),
            to_u8(b),
            to_u8(a),
        )
        .map_err(err)
}

#[op2]
pub(crate) fn op_layer_set_props(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
    #[serde] props: LayerProps,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_set_props(to_u32(id), page, props)
        .map_err(err)
}

/// KAG の [move]: id/page の現在値から to へ tween を開始する。
#[op2]
pub(crate) fn op_layer_move(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
    #[serde] args: MoveArgs,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_move(to_u32(id), page, args)
        .map_err(err)
}

/// 画像の「見える幅」を即座に決める（`layer.setCrop`）。
#[op2(fast)]
pub(crate) fn op_layer_set_crop(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
    w: f64,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_set_crop(to_u32(id), page, to_u32(w))
        .map_err(err)
}

/// 画像の「見える幅」を tween する（`layer.cropTo`）。
#[op2(fast)]
pub(crate) fn op_layer_crop_move(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
    to: f64,
    time_ms: f64,
    accel: f64,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_crop_move(
            to_u32(id),
            page,
            to_u32(to),
            to_time_ms(time_ms),
            accel as f32,
        )
        .map_err(err)
}

#[op2(fast)]
pub(crate) fn op_layer_move_cancel(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_move_cancel(to_u32(id), page)
        .map_err(err)
}

/// KAG の [move path=]（W5/B12）: 経路に沿って位置/不透明度をアニメーションする。
#[op2]
pub(crate) fn op_layer_move_path(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
    #[serde] points: Vec<MoveArgs>,
    spline: bool,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_move_path(to_u32(id), page, points, spline)
        .map_err(err)
}

#[op2]
pub(crate) fn op_layer_move_path_by_distance(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
    #[serde] points: Vec<MoveArgs>,
    spline: bool,
    total_time_ms: f64,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_move_path_by_distance(to_u32(id), page, points, spline, total_time_ms)
        .map_err(err)
}

/// 全レイヤの tween を即座に完了させる（スキップモード用）。[stopmove]（W5）も共用する。
#[op2(fast)]
pub(crate) fn op_layer_move_skip(state: &mut OpState) {
    state.borrow::<EngineState>().layer_move_skip();
}

/// いずれかのレイヤで tween が進行中か。
#[op2(fast)]
pub(crate) fn op_layer_moving(state: &mut OpState) -> bool {
    state.borrow::<EngineState>().layer_moving()
}

/// KAG の backlay。id < 0 で全レイヤ。
#[op2(fast)]
pub(crate) fn op_backlay(state: &mut OpState, id: f64) {
    state.borrow::<EngineState>().backlay(to_i32(id));
}

/// KAG の [copylay]（W3、W5 で copy_position を追加）: 別レイヤ間で内容をコピーする。
/// copy_position=false はコピー先の x/y を変えない（メッセージレイヤ宛の既知バグ修正）。
#[op2(fast)]
pub(crate) fn op_layer_copy(
    state: &mut OpState,
    src_id: f64,
    #[string] src_page: &str,
    dst_id: f64,
    #[string] dst_page: &str,
    copy_position: bool,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_copy(
            to_u32(src_id),
            src_page,
            to_u32(dst_id),
            dst_page,
            copy_position,
        )
        .map_err(err)
}

/// KAG の [button] 3 状態画像切替 / W5 [pimage] 共用（W4）: 画像ファイルの部分矩形
/// （clip 省略で全体）を既存レイヤへ (dx, dy) を左上として上書き合成する。
#[op2]
pub(crate) fn op_layer_blit_image(
    state: &mut OpState,
    id: f64,
    #[string] page: &str,
    #[string] path: String,
    dx: f64,
    dy: f64,
    #[serde] clip: Option<BlitClip>,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<EngineState>()
        .layer_blit_image(to_u32(id), page, &path, to_i32(dx), to_i32(dy), clip)
        .map_err(err)
}

/// 画面揺れ。
#[op2(fast)]
pub(crate) fn op_quake(state: &mut OpState, ax: f64, ay: f64, ms: f64) {
    state
        .borrow::<EngineState>()
        .quake(to_i32(ax), to_i32(ay), to_u32(ms));
}

/// [stopquake]（W5）: 画面揺れを即座に停止する。
#[op2(fast)]
pub(crate) fn op_quake_stop(state: &mut OpState) {
    state.borrow::<EngineState>().quake_stop();
}
