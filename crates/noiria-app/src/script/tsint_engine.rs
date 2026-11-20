//! tsint（完全自作 TypeScript インタプリタ、`../../../tsint`）スクリプトエンジン。
//!
//! `script::spawn`（V8/deno_core 版）と同じ `EngineCtx` を受け取り、同じ契約
//! （スクリプト終了時に必ず `WakeSink` へ Quit を送る）で動く、もうひとつの
//! スクリプトエンジン。ops の本体ロジックは `engine_state::EngineState` を
//! 両エンジンで共用するので、ここに書くのは:
//! - `tsint::module::ModuleHost` 実装（`noiria:` → 埋め込み TS、それ以外 →
//!   ゲームディレクトリ配下。パスジェイルは `GameDir::resolve` に委譲）
//! - `globalThis.Deno.core.ops` へのネイティブ関数配線（V8 の #[op2] 相当）
//! - tsint には無い「イベントループを回すもの」を自前実装した駆動ループ
//!   （タイマーヒープ・入力ポーリング・トランジション/オーディオのポーリング）
//!
//! だけ。
//!
//! tsint は `Rc`/`RefCell` ベースで `!Send`/`!Sync` な自作 VM のため、tokio を
//! 一切使わず素朴な OS スレッド 1 本の上で完結させる（V8 版の
//! `tokio::runtime::Builder::new_current_thread` + `LocalSet` に相当する役割を、
//! ここでは「一定間隔でポーリングするループ」が肩代わりする）。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use noiria_core::assets::GameDir;
use noiria_core::audio::channel_index;
use noiria_core::savedata::SlotInfo;
use noiria_core::text::{GlyphRect, LocateArgs, TextAlign, TextSegment, TextStyle};
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::UnboundedReceiver;
use tsint::module::{ModuleHost, ModuleOutcome};
use tsint::value::Value;
use tsint::vm::{number_to_js_string, to_number, to_string_basic, HostPromise, PumpState, Vm};
use tsint::Interpreter;

use crate::bridge::{InputEvent, UserEvent};
#[cfg(test)]
use crate::bridge::{InputSync, PendingWaits, TickSync};
#[cfg(test)]
use crate::clock::Clock;
use crate::script::engine_state::{
    video_optional_frame, BlitClip, EngineState, ImageLoadOpts, LayerProps, MoveArgs, PrintResult,
    TransArgs,
};
use crate::script::numconv::{to_coord, to_i32, to_time_ms, to_u32, to_u8, wait_duration};
use crate::script::runtime_modules::RUNTIME_MODULES;
use crate::script::EngineCtx;

// =====================================================================
// ModuleHost: `noiria:` は埋め込み TS ライブラリ（V8 版と同じテーブルを共用）、
// それ以外はゲームディレクトリ相対のファイル（相対 import にも対応）。
// =====================================================================

struct NoiriaTsHost {
    game_dir: GameDir,
}

/// `referrer_rel`（ゲームルート相対、`/` 区切り）を基準に `specifier` を
/// ゲームルート相対パスへ正規化する。`..` は素朴に 1 段階分だけ親を辿る
/// （ルートより上には出られない: 既に正規化済みの `base` は `..` を含まない
/// ため、空の状態で `pop()` しても単に無視されるだけでルート外へは脱出しない）。
fn normalize_game_rel(referrer_rel: &str, specifier: &str) -> String {
    let mut base: Vec<&str> = if specifier.starts_with("./") || specifier.starts_with("../") {
        match referrer_rel.rfind('/') {
            Some(i) => referrer_rel[..i]
                .split('/')
                .filter(|s| !s.is_empty())
                .collect(),
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };
    for comp in specifier.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                base.pop();
            }
            other => base.push(other),
        }
    }
    base.join("/")
}

impl ModuleHost for NoiriaTsHost {
    fn resolve(&self, specifier: &str, referrer: &str) -> Result<String, String> {
        if specifier.starts_with("noiria:") {
            return Ok(specifier.to_string());
        }
        let ref_rel = if referrer == "<entry>" {
            ""
        } else {
            referrer.strip_prefix("game:").unwrap_or(referrer)
        };
        Ok(format!("game:{}", normalize_game_rel(ref_rel, specifier)))
    }

    fn load(&self, resolved: &str) -> Result<String, String> {
        if resolved.starts_with("noiria:") {
            RUNTIME_MODULES
                .iter()
                .find(|(k, _)| *k == resolved)
                .map(|(_, v)| v.to_string())
                .ok_or_else(|| format!("組み込みモジュールがありません: {resolved}"))
        } else if let Some(rel) = resolved.strip_prefix("game:") {
            // パスジェイルは `GameDir::resolve`（`read_text` 内部で使用）に委譲する。
            self.game_dir.read_text(rel).map_err(|e| e.to_string())
        } else {
            Err(format!("未対応のモジュール指定子です: {resolved}"))
        }
    }
}

// =====================================================================
// Value 変換ヘルパ。V8 側の #[serde]/プリミティブ引数変換に相当する。
// =====================================================================

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

fn as_string(v: &Value) -> String {
    match v {
        Value::Str(s) => s.to_string(),
        Value::Undefined | Value::Null => String::new(),
        other => to_string_basic(other),
    }
}

fn opt_string(v: &Value) -> Option<String> {
    if v.is_nullish() {
        None
    } else {
        Some(as_string(v))
    }
}

fn num(v: &Value) -> f64 {
    to_number(v)
}

/// 整数への変換は両エンジン共通の規則（`numconv`）。
fn as_u8(v: &Value) -> u8 {
    to_u8(num(v))
}

fn as_u32(v: &Value) -> u32 {
    to_u32(num(v))
}

fn as_i32(v: &Value) -> i32 {
    to_i32(num(v))
}

fn as_coord(v: &Value) -> i32 {
    to_coord(num(v))
}

fn as_bool(v: &Value) -> bool {
    v.to_bool()
}

fn rgba_from_value(v: &Value) -> [u8; 4] {
    let items = tsint::vm::array_values(v);
    let get = |i: usize| items.get(i).cloned().unwrap_or(Value::Number(0.0));
    [
        as_u8(&get(0)),
        as_u8(&get(1)),
        as_u8(&get(2)),
        as_u8(&get(3)),
    ]
}

fn layer_props_from_value(vm: &mut Vm, v: &Value) -> Result<LayerProps, Value> {
    let x = vm.get_prop(v, "x")?;
    let y = vm.get_prop(v, "y")?;
    let z = vm.get_prop(v, "z")?;
    let opacity = vm.get_prop(v, "opacity")?;
    let visible = vm.get_prop(v, "visible")?;
    Ok(LayerProps {
        x: if x.is_nullish() {
            None
        } else {
            Some(as_coord(&x))
        },
        y: if y.is_nullish() {
            None
        } else {
            Some(as_coord(&y))
        },
        z: if z.is_nullish() {
            None
        } else {
            Some(as_i32(&z))
        },
        opacity: if opacity.is_nullish() {
            None
        } else {
            Some(as_u8(&opacity))
        },
        visible: if visible.is_nullish() {
            None
        } else {
            Some(as_bool(&visible))
        },
    })
}

fn text_style_from_value(vm: &mut Vm, v: &Value) -> Result<TextStyle, Value> {
    let mut style = TextStyle::default();
    let ff = vm.get_prop(v, "fontFamily")?;
    if !ff.is_nullish() {
        style.font_family = as_string(&ff);
    }
    let fs = vm.get_prop(v, "fontSize")?;
    if !fs.is_nullish() {
        style.font_size = num(&fs) as f32;
    }
    let lh = vm.get_prop(v, "lineHeight")?;
    if !lh.is_nullish() {
        let line_height = num(&lh) as f32;
        if !line_height.is_finite() {
            return Err(vm.type_error("lineHeight must be a finite f32"));
        }
        style.line_height = line_height;
    }
    let color = vm.get_prop(v, "color")?;
    if !color.is_nullish() {
        style.color = rgba_from_value(&color);
    }
    let oc = vm.get_prop(v, "outlineColor")?;
    if !oc.is_nullish() {
        style.outline_color = rgba_from_value(&oc);
    }
    let ow = vm.get_prop(v, "outlineWidth")?;
    if !ow.is_nullish() {
        style.outline_width = as_u32(&ow);
    }
    let bw = vm.get_prop(v, "boxW")?;
    if !bw.is_nullish() {
        style.box_w = as_u32(&bw);
    }
    let bh = vm.get_prop(v, "boxH")?;
    if !bh.is_nullish() {
        style.box_h = as_u32(&bh);
    }
    let mx = vm.get_prop(v, "marginX")?;
    if !mx.is_nullish() {
        style.margin_x = as_i32(&mx);
    }
    let my = vm.get_prop(v, "marginY")?;
    if !my.is_nullish() {
        style.margin_y = as_i32(&my);
    }
    let align = vm.get_prop(v, "align")?;
    if !align.is_nullish() {
        style.align = match as_string(&align).as_str() {
            "left" => TextAlign::Left,
            "center" => TextAlign::Center,
            "right" => TextAlign::Right,
            value => {
                return Err(vm.type_error(format!(
                    "unknown align `{value}`; expected left, center, or right"
                )));
            }
        };
    }
    Ok(style)
}

fn segments_from_value(vm: &mut Vm, v: &Value) -> Result<Vec<TextSegment>, Value> {
    let arr = tsint::vm::array_values(v);
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let text = vm.get_prop(&item, "text")?;
        let color = vm.get_prop(&item, "color")?;
        let size = vm.get_prop(&item, "size")?;
        let bold = vm.get_prop(&item, "bold")?;
        let edge_color = vm.get_prop(&item, "edgeColor")?;
        let ruby = vm.get_prop(&item, "ruby")?;
        let locate = vm.get_prop(&item, "locate")?;
        let indent = vm.get_prop(&item, "indent")?;
        let inline_image = vm.get_prop(&item, "inlineImage")?;
        let font_family = vm.get_prop(&item, "fontFamily")?;
        let locate = if locate.is_nullish() {
            None
        } else {
            let x = vm.get_prop(&locate, "x")?;
            let y = vm.get_prop(&locate, "y")?;
            Some(LocateArgs {
                x: num(&x) as f32,
                y: if y.is_nullish() {
                    None
                } else {
                    Some(num(&y) as f32)
                },
            })
        };
        out.push(TextSegment {
            text: as_string(&text),
            color: if color.is_nullish() {
                None
            } else {
                Some(rgba_from_value(&color))
            },
            size: if size.is_nullish() {
                None
            } else {
                Some(num(&size) as f32)
            },
            bold: if bold.is_nullish() {
                None
            } else {
                Some(as_bool(&bold))
            },
            edge_color: if edge_color.is_nullish() {
                None
            } else {
                Some(rgba_from_value(&edge_color))
            },
            ruby: opt_string(&ruby),
            locate,
            indent: if indent.is_nullish() {
                None
            } else {
                Some(as_bool(&indent))
            },
            inline_image: opt_string(&inline_image),
            font_family: opt_string(&font_family),
        });
    }
    Ok(out)
}

fn move_args_from_value(vm: &mut Vm, v: &Value) -> Result<MoveArgs, Value> {
    let to_x = vm.get_prop(v, "toX")?;
    let to_y = vm.get_prop(v, "toY")?;
    let to_opacity = vm.get_prop(v, "toOpacity")?;
    let time_ms = vm.get_prop(v, "timeMs")?;
    let accel = vm.get_prop(v, "accel")?;
    Ok(MoveArgs {
        to_x: if to_x.is_nullish() {
            None
        } else {
            Some(as_coord(&to_x))
        },
        to_y: if to_y.is_nullish() {
            None
        } else {
            Some(as_coord(&to_y))
        },
        to_opacity: if to_opacity.is_nullish() {
            None
        } else {
            Some(as_u8(&to_opacity))
        },
        time_ms: if time_ms.is_nullish() {
            0
        } else {
            to_time_ms(num(&time_ms))
        },
        accel: if accel.is_nullish() {
            0.0
        } else {
            num(&accel) as f32
        },
    })
}

/// `layer_blit_image`（W4）の clip= 引数: `{x,y,w,h}` オブジェクトまたは
/// undefined/null（画像全体）。
fn blit_clip_from_value(vm: &mut Vm, v: &Value) -> Result<Option<BlitClip>, Value> {
    if v.is_nullish() {
        return Ok(None);
    }
    let x = vm.get_prop(v, "x")?;
    let y = vm.get_prop(v, "y")?;
    let w = vm.get_prop(v, "w")?;
    let h = vm.get_prop(v, "h")?;
    Ok(Some(BlitClip {
        x: as_u32(&x),
        y: as_u32(&y),
        w: as_u32(&w),
        h: as_u32(&h),
    }))
}

fn trans_args_from_value(vm: &mut Vm, v: &Value) -> Result<TransArgs, Value> {
    let method = as_string(&vm.get_prop(v, "method")?);
    let time_ms = to_time_ms(num(&vm.get_prop(v, "timeMs")?));
    let rule = opt_string(&vm.get_prop(v, "rule")?);
    let vague_v = vm.get_prop(v, "vague")?;
    let vague = if vague_v.is_nullish() {
        None
    } else {
        Some(num(&vague_v) as f32)
    };
    let from = opt_string(&vm.get_prop(v, "from")?);
    let stay = opt_string(&vm.get_prop(v, "stay")?);
    Ok(TransArgs {
        method,
        time_ms,
        rule,
        vague,
        from,
        stay,
    })
}

/// `layer_move_path`（W5）の points= 引数: 各要素が `move_args_from_value` と同じ形の配列。
fn move_args_array_from_value(vm: &mut Vm, v: &Value) -> Result<Vec<MoveArgs>, Value> {
    let arr = tsint::vm::array_values(v);
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(move_args_from_value(vm, &item)?);
    }
    Ok(out)
}

/// [image]（W5 拡張）の opts= 引数: `{clip, flipH, flipV, grayscale}`。
fn image_load_opts_from_value(vm: &mut Vm, v: &Value) -> Result<ImageLoadOpts, Value> {
    let clip_v = vm.get_prop(v, "clip")?;
    let clip = blit_clip_from_value(vm, &clip_v)?;
    let flip_h = as_bool(&vm.get_prop(v, "flipH")?);
    let flip_v = as_bool(&vm.get_prop(v, "flipV")?);
    let grayscale = as_bool(&vm.get_prop(v, "grayscale")?);
    Ok(ImageLoadOpts {
        clip,
        flip_h,
        flip_v,
        grayscale,
    })
}

fn obj_from_pairs(pairs: &[(&str, Value)]) -> Value {
    let o = Value::new_plain_object();
    if let Value::Object(rc) = &o {
        let mut b = rc.borrow_mut();
        for (k, v) in pairs {
            b.props.set_value(Rc::from(*k), v.clone());
        }
    }
    o
}

fn print_result_to_value(r: PrintResult) -> Value {
    obj_from_pairs(&[
        ("total", Value::Number(r.total as f64)),
        ("added", Value::Number(r.added as f64)),
    ])
}

fn glyph_rects_to_value(rects: Vec<GlyphRect>) -> Value {
    Value::new_array(
        rects
            .into_iter()
            .map(|g| {
                obj_from_pairs(&[
                    ("x", Value::Number(g.x as f64)),
                    ("y", Value::Number(g.y as f64)),
                    ("w", Value::Number(g.w as f64)),
                    ("h", Value::Number(g.h as f64)),
                    ("byte_start", Value::Number(g.byte_start as f64)),
                    ("byte_end", Value::Number(g.byte_end as f64)),
                    ("line_i", Value::Number(g.line_i as f64)),
                ])
            })
            .collect(),
    )
}

fn slot_list_to_value(list: Vec<SlotInfo>) -> Value {
    Value::new_array(
        list.into_iter()
            .map(|s| obj_from_pairs(&[("slot", Value::str(s.slot)), ("json", Value::str(s.json))]))
            .collect(),
    )
}

fn input_event_to_value(ev: &InputEvent) -> Value {
    let obj = obj_from_pairs(&[
        ("kind", Value::str(ev.kind)),
        ("x", Value::Number(ev.x)),
        ("y", Value::Number(ev.y)),
        ("t", Value::Number(ev.t)),
    ]);
    if let Value::Object(o) = &obj {
        let mut b = o.borrow_mut();
        b.props.set_value(
            Rc::from("button"),
            match ev.button {
                Some(s) => Value::str(s),
                None => Value::Undefined,
            },
        );
        b.props.set_value(
            Rc::from("key"),
            match &ev.key {
                Some(s) => Value::str(s.clone()),
                None => Value::Undefined,
            },
        );
        b.props.set_value(
            Rc::from("delta"),
            match ev.delta {
                Some(d) => Value::Number(d),
                None => Value::Undefined,
            },
        );
    }
    obj
}

fn quit_event_value() -> Value {
    input_event_to_value(&InputEvent {
        kind: "quit",
        x: 0.0,
        y: 0.0,
        button: None,
        key: None,
        delta: None,
        t: 0.0,
    })
}

/// 未捕捉の Promise reject を大まかに文字列化する（ログ用。V8 版の
/// `to_display_string` ほど厳密である必要はない）。
fn debug_value(v: &Value) -> String {
    match v {
        Value::Str(s) => s.to_string(),
        Value::Number(n) => number_to_js_string(*n),
        Value::Bool(b) => b.to_string(),
        Value::Undefined => "undefined".to_string(),
        Value::Null => "null".to_string(),
        Value::Object(_) => "[object]".to_string(),
        Value::Uninit => "<uninit>".to_string(),
    }
}

fn sync_native(
    name: &'static str,
    arity: usize,
    f: impl Fn(&mut Vm, Value, &[Value]) -> Result<Value, Value> + 'static,
) -> Value {
    Value::native_fn(name, arity, Rc::new(f))
}

fn async_native(
    name: &'static str,
    arity: usize,
    handler: impl Fn(&mut Vm, Value, &[Value], HostPromise) + 'static,
) -> Value {
    Value::native_fn(
        name,
        arity,
        Rc::new(move |vm, this, args| {
            let (promise_val, handle) = vm.new_host_promise();
            handler(vm, this, args, handle);
            Ok(promise_val)
        }),
    )
}

// =====================================================================
// 非同期 ops の駆動状態。tsint にはタイマー/IO が無いので、ここに溜めた
// 保留中の `HostPromise` を専用スレッドの駆動ループが期限/ポーリングで
// 解決していく。
// =====================================================================

#[derive(Default)]
struct PendingOps {
    /// `op_wait` の期限付きタイマー。
    timers: Vec<(Instant, HostPromise)>,
    /// `op_trans_wait` の完了待ち。
    trans: Vec<HostPromise>,
    /// `op_audio_wait_end` の完了待ち: (チャンネル index, handle)。
    audio: Vec<(usize, HostPromise)>,
    /// `op_input_next_event` の保留中ハンドル（同時に 1 個だけという前提は V8 版と同じ）。
    input: Option<HostPromise>,
    /// メインスレッドから届いた未消費の入力イベント。
    input_buffer: VecDeque<InputEvent>,
    /// 入力チャンネルが閉じた（メインスレッド側の送信口が drop された）。
    input_closed: bool,
    /// ヘッドレスのロックステップ同期用: 現在保留中の `input` ハンドルに対して
    /// `EngineState::input_sync.ack()` を出したか。ハンドル登録時に false へ戻し、
    /// `drive()` が「pump が Idle（マイクロタスク消化済み）かつ入力バッファが空」
    /// という静止条件を観測した最初の反復で ack して true にする
    /// （V8 版 `run_script` のイベントループラッパと同じ意味の同期点）。
    input_acked: bool,
}

/// ops をインストールし、`globalThis.Deno.core.ops` へ生やす。
fn install_ops(
    interp: &mut Interpreter,
    es: &Rc<RefCell<EngineState>>,
    pending: &Rc<RefCell<PendingOps>>,
) {
    let ops_obj = Value::new_plain_object();
    macro_rules! op {
        ($name:literal, $value:expr) => {
            if let Value::Object(o) = &ops_obj {
                o.borrow_mut().props.set_value(Rc::from($name), $value);
            }
        };
    }

    // ---- layer ----
    {
        let es = es.clone();
        op!(
            "op_layer_new",
            sync_native("op_layer_new", 1, move |vm, _this, args| {
                let kind = as_string(&arg(args, 0));
                es.borrow()
                    .layer_new(&kind)
                    .map(|id| Value::Number(id as f64))
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_free",
            sync_native("op_layer_free", 1, move |_vm, _this, args| {
                es.borrow().layer_free(as_u32(&arg(args, 0)));
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_load_image",
            sync_native("op_layer_load_image", 3, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                let path = as_string(&arg(args, 2));
                es.borrow()
                    .layer_load_image(id, &page, path)
                    .map(|(w, h)| {
                        Value::new_array(vec![Value::Number(w as f64), Value::Number(h as f64)])
                    })
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_load_image_ex",
            sync_native("op_layer_load_image_ex", 4, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                let path = as_string(&arg(args, 2));
                let opts = image_load_opts_from_value(vm, &arg(args, 3))?;
                es.borrow()
                    .layer_load_image_ex(id, &page, path, opts)
                    .map(|(w, h)| {
                        Value::new_array(vec![Value::Number(w as f64), Value::Number(h as f64)])
                    })
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_clear_image",
            sync_native("op_layer_clear_image", 2, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                es.borrow()
                    .layer_clear_image(id, &page)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_fill",
            sync_native("op_layer_fill", 8, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                let w = as_u32(&arg(args, 2));
                let h = as_u32(&arg(args, 3));
                let r = as_u8(&arg(args, 4));
                let g = as_u8(&arg(args, 5));
                let b = as_u8(&arg(args, 6));
                let a = as_u8(&arg(args, 7));
                es.borrow()
                    .layer_fill(id, &page, w, h, r, g, b, a)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_msgbox",
            sync_native("op_layer_msgbox", 9, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                let w = as_u32(&arg(args, 2));
                let h = as_u32(&arg(args, 3));
                let radius = as_u32(&arg(args, 4));
                let r = as_u8(&arg(args, 5));
                let g = as_u8(&arg(args, 6));
                let b = as_u8(&arg(args, 7));
                let a = as_u8(&arg(args, 8));
                es.borrow()
                    .layer_msgbox(id, &page, w, h, radius, r, g, b, a)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_panel",
            sync_native("op_layer_panel", 3, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                let spec: Vec<f64> = tsint::vm::array_values(&arg(args, 2))
                    .iter()
                    .map(num)
                    .collect();
                es.borrow()
                    .layer_panel(id, &page, &spec)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_load_image_scaled",
            sync_native("op_layer_load_image_scaled", 5, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                let path = as_string(&arg(args, 2));
                let w = as_u32(&arg(args, 3));
                let h = as_u32(&arg(args, 4));
                es.borrow()
                    .layer_load_image_scaled(id, &page, path, w, h)
                    .map(|(w, h)| {
                        Value::new_array(vec![Value::Number(w as f64), Value::Number(h as f64)])
                    })
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_set_props",
            sync_native("op_layer_set_props", 3, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                let props = layer_props_from_value(vm, &arg(args, 2))?;
                es.borrow()
                    .layer_set_props(id, &page, props)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_move",
            sync_native("op_layer_move", 3, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                let move_args = move_args_from_value(vm, &arg(args, 2))?;
                es.borrow()
                    .layer_move(id, &page, move_args)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_set_crop",
            sync_native("op_layer_set_crop", 3, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                let w = as_u32(&arg(args, 2));
                es.borrow()
                    .layer_set_crop(id, &page, w)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_crop_move",
            sync_native("op_layer_crop_move", 5, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                let to = as_u32(&arg(args, 2));
                let time_ms = to_time_ms(num(&arg(args, 3)));
                let accel = num(&arg(args, 4)) as f32;
                es.borrow()
                    .layer_crop_move(id, &page, to, time_ms, accel)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_move_cancel",
            sync_native("op_layer_move_cancel", 2, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                es.borrow()
                    .layer_move_cancel(id, &page)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_move_path",
            sync_native("op_layer_move_path", 4, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                let points = move_args_array_from_value(vm, &arg(args, 2))?;
                let spline = as_bool(&arg(args, 3));
                es.borrow()
                    .layer_move_path(id, &page, points, spline)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_move_path_by_distance",
            sync_native(
                "op_layer_move_path_by_distance",
                5,
                move |vm, _this, args| {
                    let id = as_u32(&arg(args, 0));
                    let page = as_string(&arg(args, 1));
                    let points = move_args_array_from_value(vm, &arg(args, 2))?;
                    let spline = as_bool(&arg(args, 3));
                    let total_time_ms = num(&arg(args, 4));
                    es.borrow()
                        .layer_move_path_by_distance(id, &page, points, spline, total_time_ms)
                        .map(|_| Value::Undefined)
                        .map_err(|e| vm.make_error("Error", e))
                }
            )
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_move_skip",
            sync_native("op_layer_move_skip", 0, move |_vm, _this, _args| {
                es.borrow().layer_move_skip();
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_moving",
            sync_native("op_layer_moving", 0, move |_vm, _this, _args| {
                Ok(Value::Bool(es.borrow().layer_moving()))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_backlay",
            sync_native("op_backlay", 1, move |_vm, _this, args| {
                es.borrow().backlay(as_i32(&arg(args, 0)));
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_copy",
            sync_native("op_layer_copy", 5, move |vm, _this, args| {
                let src_id = as_u32(&arg(args, 0));
                let src_page = as_string(&arg(args, 1));
                let dst_id = as_u32(&arg(args, 2));
                let dst_page = as_string(&arg(args, 3));
                let copy_position = as_bool(&arg(args, 4));
                es.borrow()
                    .layer_copy(src_id, &src_page, dst_id, &dst_page, copy_position)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_blit_image",
            sync_native("op_layer_blit_image", 6, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                let path = as_string(&arg(args, 2));
                let dx = as_i32(&arg(args, 3));
                let dy = as_i32(&arg(args, 4));
                let clip = blit_clip_from_value(vm, &arg(args, 5))?;
                es.borrow()
                    .layer_blit_image(id, &page, &path, dx, dy, clip)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_quake",
            sync_native("op_quake", 3, move |_vm, _this, args| {
                es.borrow().quake(
                    as_i32(&arg(args, 0)),
                    as_i32(&arg(args, 1)),
                    as_u32(&arg(args, 2)),
                );
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_quake_stop",
            sync_native("op_quake_stop", 0, move |_vm, _this, _args| {
                es.borrow().quake_stop();
                Ok(Value::Undefined)
            })
        );
    }

    // ---- text ----
    {
        let es = es.clone();
        op!(
            "op_text_style",
            sync_native("op_text_style", 2, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let style = text_style_from_value(vm, &arg(args, 1))?;
                es.borrow_mut().text_style(id, style);
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_text_print",
            sync_native("op_text_print", 4, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let segments = segments_from_value(vm, &arg(args, 1))?;
                let append = as_bool(&arg(args, 2));
                let cps = num(&arg(args, 3));
                es.borrow_mut()
                    .text_print(id, segments, append, cps)
                    .map(print_result_to_value)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_text_reflow",
            sync_native("op_text_reflow", 1, move |vm, _this, args| {
                es.borrow_mut()
                    .text_reflow(as_u32(&arg(args, 0)))
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_text_complete",
            sync_native("op_text_complete", 1, move |_vm, _this, args| {
                es.borrow().text_complete(as_u32(&arg(args, 0)));
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_text_is_done",
            sync_native("op_text_is_done", 1, move |_vm, _this, args| {
                Ok(Value::Bool(es.borrow().text_is_done(as_u32(&arg(args, 0)))))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_text_clear",
            sync_native("op_text_clear", 1, move |_vm, _this, args| {
                es.borrow_mut().text_clear(as_u32(&arg(args, 0)));
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_text_rects",
            sync_native("op_text_rects", 1, move |_vm, _this, args| {
                Ok(glyph_rects_to_value(
                    es.borrow().text_rects(as_u32(&arg(args, 0))),
                ))
            })
        );
    }

    // ---- transition ----
    {
        let es = es.clone();
        op!(
            "op_trans_begin",
            sync_native("op_trans_begin", 1, move |vm, _this, args| {
                let a = trans_args_from_value(vm, &arg(args, 0))?;
                es.borrow()
                    .trans_begin(a)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_trans_skip",
            sync_native("op_trans_skip", 0, move |_vm, _this, _args| {
                es.borrow().trans_skip();
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        let pending = pending.clone();
        op!(
            "op_trans_wait",
            async_native("op_trans_wait", 0, move |vm, _this, _args, handle| {
                if es.borrow().trans_active() {
                    pending.borrow_mut().trans.push(handle);
                } else {
                    handle.resolve(vm, Value::Undefined);
                }
            })
        );
    }

    // ---- audio ----
    {
        let es = es.clone();
        op!(
            "op_audio_play",
            sync_native("op_audio_play", 7, move |vm, _this, args| {
                let ch = as_string(&arg(args, 0));
                let path = as_string(&arg(args, 1));
                let looping = as_bool(&arg(args, 2));
                let volume = num(&arg(args, 3));
                let fade_in_ms = as_u32(&arg(args, 4));
                let start_position_ms = num(&arg(args, 5));
                let start_paused = as_bool(&arg(args, 6));
                es.borrow()
                    .audio_play(
                        &ch,
                        &path,
                        looping,
                        volume,
                        fade_in_ms,
                        start_position_ms,
                        start_paused,
                    )
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_audio_stop",
            sync_native("op_audio_stop", 2, move |vm, _this, args| {
                let ch = as_string(&arg(args, 0));
                es.borrow()
                    .audio_stop(&ch, as_u32(&arg(args, 1)))
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_audio_position",
            sync_native("op_audio_position", 1, move |vm, _this, args| {
                let ch = as_string(&arg(args, 0));
                es.borrow()
                    .audio_position(&ch)
                    .map(|position| Value::Number(position as f64))
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_audio_volume",
            sync_native("op_audio_volume", 2, move |vm, _this, args| {
                let ch = as_string(&arg(args, 0));
                es.borrow()
                    .audio_volume(&ch, num(&arg(args, 1)))
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_audio_is_playing",
            sync_native("op_audio_is_playing", 1, move |vm, _this, args| {
                let ch = as_string(&arg(args, 0));
                es.borrow()
                    .audio_is_playing(&ch)
                    .map(Value::Bool)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_audio_pause",
            sync_native("op_audio_pause", 1, move |vm, _this, args| {
                let ch = as_string(&arg(args, 0));
                es.borrow()
                    .audio_pause(&ch)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_audio_resume",
            sync_native("op_audio_resume", 1, move |vm, _this, args| {
                let ch = as_string(&arg(args, 0));
                es.borrow()
                    .audio_resume(&ch)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_audio_fade_to",
            sync_native("op_audio_fade_to", 4, move |vm, _this, args| {
                let ch = as_string(&arg(args, 0));
                let volume = num(&arg(args, 1));
                let ms = as_u32(&arg(args, 2));
                let then = as_string(&arg(args, 3));
                es.borrow()
                    .audio_fade_to(&ch, volume, ms, &then)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_audio_is_fading",
            sync_native("op_audio_is_fading", 1, move |vm, _this, args| {
                let ch = as_string(&arg(args, 0));
                es.borrow()
                    .audio_is_fading(&ch)
                    .map(Value::Bool)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_audio_is_finished",
            sync_native("op_audio_is_finished", 1, move |vm, _this, args| {
                let ch = as_string(&arg(args, 0));
                es.borrow()
                    .audio_is_finished(&ch)
                    .map(Value::Bool)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let pending = pending.clone();
        op!(
            "op_audio_wait_end",
            async_native("op_audio_wait_end", 1, move |vm, _this, args, handle| {
                let ch_name = as_string(&arg(args, 0));
                match channel_index(&ch_name) {
                    Some(ch) => pending.borrow_mut().audio.push((ch, handle)),
                    None => {
                        let e = vm.make_error("Error", format!("不明なチャンネル: {ch_name}"));
                        handle.reject(vm, e);
                    }
                }
            })
        );
    }

    // ---- video ----
    {
        let es = es.clone();
        op!(
            "op_video_play",
            sync_native("op_video_play", 3, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let path = as_string(&arg(args, 1));
                let looping = as_bool(&arg(args, 2));
                es.borrow_mut()
                    .video_play(id, &path, looping)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_play_at",
            sync_native("op_video_play_at", 5, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let path = as_string(&arg(args, 1));
                let looping = as_bool(&arg(args, 2));
                let frame = video_optional_frame(num(&arg(args, 3)))
                    .map_err(|e| vm.make_error("Error", e))?;
                let position_ms = num(&arg(args, 4));
                es.borrow_mut()
                    .video_play_at(
                        id,
                        &path,
                        looping,
                        frame,
                        (position_ms != -1.0).then_some(position_ms),
                    )
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_stop",
            sync_native("op_video_stop", 0, move |_vm, _this, _args| {
                es.borrow_mut().video_stop();
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_is_playing",
            sync_native("op_video_is_playing", 0, move |_vm, _this, _args| {
                Ok(Value::Bool(es.borrow().video_is_playing()))
            })
        );
    }
    // W6: openvideo/preparevideo/playvideo/pausevideo/resumevideo/rewindvideo
    {
        let es = es.clone();
        op!(
            "op_video_open",
            sync_native("op_video_open", 3, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let path = as_string(&arg(args, 1));
                let looping = as_bool(&arg(args, 2));
                es.borrow_mut()
                    .video_open(id, &path, looping)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_prepare",
            sync_native("op_video_prepare", 3, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let path = as_string(&arg(args, 1));
                let looping = as_bool(&arg(args, 2));
                es.borrow_mut()
                    .video_prepare(id, &path, looping)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_prepare_opened",
            sync_native("op_video_prepare_opened", 1, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                es.borrow_mut()
                    .video_prepare_opened(id)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_play_prepared",
            sync_native("op_video_play_prepared", 1, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                es.borrow_mut()
                    .video_play_prepared(id)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_pause",
            sync_native("op_video_pause", 0, move |_vm, _this, _args| {
                es.borrow_mut().video_pause();
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_resume",
            sync_native("op_video_resume", 0, move |_vm, _this, _args| {
                es.borrow_mut().video_resume();
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_rewind",
            sync_native("op_video_rewind", 0, move |_vm, _this, _args| {
                es.borrow_mut().video_rewind();
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_is_paused",
            sync_native("op_video_is_paused", 0, move |_vm, _this, _args| {
                Ok(Value::Bool(es.borrow().video_is_paused()))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_seek_frame",
            sync_native("op_video_seek_frame", 1, move |vm, _this, args| {
                es.borrow_mut()
                    .video_seek_frame(as_u32(&arg(args, 0)) as usize)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_seek_position",
            sync_native("op_video_seek_position", 1, move |vm, _this, args| {
                es.borrow_mut()
                    .video_seek_position(num(&arg(args, 0)))
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_set_period_event",
            sync_native("op_video_set_period_event", 1, move |_vm, _this, args| {
                es.borrow_mut()
                    .video_set_period_event(as_u32(&arg(args, 0)) as usize);
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_cancel_period_event",
            sync_native(
                "op_video_cancel_period_event",
                0,
                move |_vm, _this, _args| {
                    es.borrow_mut().video_cancel_period_event();
                    Ok(Value::Undefined)
                }
            )
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_set_segment_loop",
            sync_native("op_video_set_segment_loop", 2, move |vm, _this, args| {
                let start = as_u32(&arg(args, 0)) as usize;
                let end = as_u32(&arg(args, 1)) as usize;
                es.borrow_mut()
                    .video_set_segment_loop(start, end)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_cancel_segment_loop",
            sync_native(
                "op_video_cancel_segment_loop",
                0,
                move |_vm, _this, _args| {
                    es.borrow_mut().video_cancel_segment_loop();
                    Ok(Value::Undefined)
                }
            )
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_take_event",
            sync_native("op_video_take_event", 1, move |_vm, _this, args| {
                let kind = as_string(&arg(args, 0));
                Ok(Value::Bool(es.borrow_mut().video_take_event(&kind)))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_video_generation",
            sync_native("op_video_generation", 0, move |_vm, _this, _args| {
                Ok(Value::Number(es.borrow().video_generation()))
            })
        );
    }

    // ---- input ----
    {
        let pending = pending.clone();
        op!(
            "op_input_next_event",
            async_native("op_input_next_event", 0, move |vm, _this, _args, handle| {
                let mut p = pending.borrow_mut();
                if let Some(ev) = p.input_buffer.pop_front() {
                    drop(p);
                    handle.resolve(vm, input_event_to_value(&ev));
                } else if p.input_closed {
                    drop(p);
                    handle.resolve(vm, quit_event_value());
                } else {
                    // ヘッドレスのロックステップ同期: 新しい入力待ちハンドルの登録。
                    // ack は `drive()` が静止条件（pump Idle）を確認してから出す
                    // （`PendingOps::input_acked` のドキュメント参照）。
                    p.input = Some(handle);
                    p.input_acked = false;
                }
            })
        );
    }

    // ---- sys ----
    {
        let es = es.clone();
        op!(
            "op_log",
            sync_native("op_log", 2, move |_vm, _this, args| {
                let level = as_string(&arg(args, 0));
                let msg = as_string(&arg(args, 1));
                es.borrow().sys_log(&level, &msg);
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_now",
            sync_native("op_now", 0, move |_vm, _this, _args| {
                Ok(Value::Number(es.borrow().now()))
            })
        );
    }
    {
        let pending = pending.clone();
        let es = es.clone();
        op!(
            "op_wait",
            async_native("op_wait", 1, move |_vm, _this, args, handle| {
                let ms = num(&arg(args, 0));
                // 期限は EngineState::clock 基準: windowed（Clock::Real）は従来どおり
                // 実時間、ヘッドレス（Clock::Virtual）は仮想時刻に対して決定的になる
                // （V8 版 ops_sys::op_wait と同じ方針。`docs/implementation-notes.md`
                // 「2026-07-13 headless ロックステップ完全決定論化」参照）。
                let deadline =
                    es.borrow().clock.now() + wait_duration(ms);
                pending.borrow_mut().timers.push((deadline, handle));
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_quit",
            sync_native("op_quit", 0, move |_vm, _this, _args| {
                es.borrow().quit();
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_set_title",
            sync_native("op_set_title", 1, move |_vm, _this, args| {
                es.borrow().set_title(as_string(&arg(args, 0)));
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_read_text",
            sync_native("op_read_text", 1, move |vm, _this, args| {
                let rel = as_string(&arg(args, 0));
                es.borrow()
                    .read_text(&rel)
                    .map(Value::str)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_file_exists",
            sync_native("op_file_exists", 1, move |_vm, _this, args| {
                Ok(Value::Bool(
                    es.borrow().file_exists(&as_string(&arg(args, 0))),
                ))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_save_write",
            sync_native("op_save_write", 2, move |vm, _this, args| {
                let slot = as_string(&arg(args, 0));
                let json = as_string(&arg(args, 1));
                es.borrow()
                    .save_write(&slot, &json)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_save_read",
            sync_native("op_save_read", 1, move |vm, _this, args| {
                let slot = as_string(&arg(args, 0));
                es.borrow()
                    .save_read(&slot)
                    .map(|opt| match opt {
                        Some(s) => Value::str(s),
                        None => Value::Null,
                    })
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_save_list",
            sync_native("op_save_list", 0, move |vm, _this, _args| {
                es.borrow()
                    .save_list()
                    .map(slot_list_to_value)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_save_delete",
            sync_native("op_save_delete", 1, move |vm, _this, args| {
                let slot = as_string(&arg(args, 0));
                es.borrow()
                    .save_delete(&slot)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_screenshot",
            sync_native("op_screenshot", 1, move |vm, _this, args| {
                let slot = as_string(&arg(args, 0));
                es.borrow()
                    .screenshot(&slot)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }

    {
        let es = es.clone();
        op!(
            "op_thumb_capture",
            sync_native("op_thumb_capture", 2, move |vm, _this, args| {
                es.borrow()
                    .thumb_capture_sized(as_u32(&arg(args, 0)), as_u32(&arg(args, 1)))
                    .map(|()| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_prefetch_save_thumbs",
            sync_native("op_prefetch_save_thumbs", 2, move |_vm, _this, args| {
                es.borrow()
                    .prefetch_save_thumbs(as_u32(&arg(args, 0)), as_u32(&arg(args, 1)));
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_prefetch_images_scaled",
            sync_native("op_prefetch_images_scaled", 3, move |_vm, _this, args| {
                let list = as_string(&arg(args, 0))
                    .split('\u{a}')
                    .filter(|p| !p.is_empty())
                    .map(str::to_string)
                    .collect();
                es.borrow()
                    .prefetch_images_scaled(list, as_u32(&arg(args, 1)), as_u32(&arg(args, 2)));
                Ok(Value::Undefined)
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_thumb_save",
            sync_native("op_thumb_save", 1, move |vm, _this, args| {
                let slot = as_string(&arg(args, 0));
                es.borrow()
                    .thumb_save(&slot)
                    .map(|_| Value::Undefined)
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_wall_clock_string",
            sync_native("op_wall_clock_string", 0, move |_vm, _this, _args| {
                Ok(Value::str(es.borrow().wall_clock_string()))
            })
        );
    }
    {
        let es = es.clone();
        op!(
            "op_layer_load_save_thumb",
            sync_native("op_layer_load_save_thumb", 5, move |vm, _this, args| {
                let id = as_u32(&arg(args, 0));
                let page = as_string(&arg(args, 1));
                let slot = as_string(&arg(args, 2));
                let w = as_u32(&arg(args, 3));
                let h = as_u32(&arg(args, 4));
                es.borrow()
                    .layer_load_save_thumb_sized(id, &page, &slot, w, h)
                    .map(|(w, h)| {
                        Value::new_array(vec![Value::Number(w as f64), Value::Number(h as f64)])
                    })
                    .map_err(|e| vm.make_error("Error", e))
            })
        );
    }

    // `globalThis.Deno.core.ops = ops_obj`
    let core_obj = Value::new_plain_object();
    if let Value::Object(o) = &core_obj {
        o.borrow_mut().props.set_value(Rc::from("ops"), ops_obj);
    }
    let deno_obj = Value::new_plain_object();
    if let Value::Object(o) = &deno_obj {
        o.borrow_mut().props.set_value(Rc::from("core"), core_obj);
    }
    let global_this = interp
        .vm()
        .globals
        .get("globalThis")
        .expect("globalThis がビルトインとして登録されているはず")
        .borrow()
        .clone();
    if let Value::Object(o) = &global_this {
        o.borrow_mut().props.set_value(Rc::from("Deno"), deno_obj);
    } else {
        panic!("globalThis がオブジェクトではありません");
    }
}

// =====================================================================
// スクリプトスレッド本体
// =====================================================================

/// スクリプトスレッドを起動する。スクリプト終了（正常/異常とも）でメインループへ Quit を送る
/// （V8 版 `script::spawn` と同じ契約）。
pub fn spawn(ctx: EngineCtx) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("noiria-script-tsint".into())
        .spawn(move || {
            let wake = ctx.proxy.clone();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_script(ctx))) {
                Ok(Ok(())) => {}
                Ok(Err(e)) => crate::fatal::record_script_error(format!("{e} (tsint)")),
                Err(p) => crate::fatal::record_script_error(crate::fatal::panic_message(&*p)),
            }
            wake.send(UserEvent::Quit);
        })
        .expect("スクリプトスレッド(tsint)の起動に失敗")
}

/// エンジン本体。tokio を使わないので同期関数のまま完結する。
fn run_script(ctx: EngineCtx) -> Result<(), String> {
    run_script_counted(ctx, None)
}

/// `run_script` の計測つき版。`wakes` があれば、`drive` が 1 周するたびに加算する（待機中の起床回数の測定用）。
fn run_script_counted(ctx: EngineCtx, wakes: Option<&AtomicU64>) -> Result<(), String> {
    let EngineCtx {
        scene,
        proxy,
        input_rx,
        trans_notify,
        audio_tx,
        audio_shared,
        game_dir,
        config,
        start,
        clock,
        input_sync,
        tick_sync,
        pending_waits,
        op_log,
    } = ctx;
    let main_rel = config.main.clone();
    let host = NoiriaTsHost {
        game_dir: game_dir.clone(),
    };

    let es = Rc::new(RefCell::new(EngineState::new(
        scene,
        proxy,
        trans_notify,
        audio_tx,
        audio_shared,
        game_dir,
        config,
        start,
        clock,
        input_sync,
        tick_sync,
        pending_waits,
        op_log,
    )));
    let pending = Rc::new(RefCell::new(PendingOps::default()));

    let mut interp = Interpreter::new();
    interp.set_now_fn(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    });
    interp.set_console_hook(|s| log::info!("[tsint console] {s}"));
    interp.set_unhandled_rejection_hook(|v| {
        log::warn!("[tsint] 未処理の Promise reject: {}", debug_value(v))
    });

    // CJK フォントの起動時チェック（失敗しても続行、ログのみ。V8 版と同じ挙動）。
    {
        let style = noiria_core::text::TextStyle::default();
        if let Err(e) = es.borrow_mut().texts.engine.assert_cjk(&style) {
            log::warn!("{e:#}");
        }
    }

    install_ops(&mut interp, &es, &pending);

    let outcome = interp
        .run_module_graph(&main_rel, &host)
        .map_err(|e| e.to_string())?;
    match outcome {
        ModuleOutcome::Settled(_) => {
            log::info!("tsint: メインモジュールの評価が完了しました（await 待ちなし）");
            Ok(())
        }
        ModuleOutcome::Pending => {
            let real = es.borrow().clock.is_real();
            let trans_notify = es.borrow().trans_notify.clone();
            let intake = if real {
                Intake::forwarded(input_rx, trans_notify)
            } else {
                Intake::Direct(input_rx)
            };
            drive(&mut interp, &es, &pending, intake, wakes)
        }
    }
}

/// ヘッドレス（仮想クロック）で `drive` がアイドルポーリングする上限間隔。
/// headless.rs のフレーム刻み（16ms、`crate::headless::TICK`）より十分小さくして、
/// `--autotest` の move/click の取りこぼし〜処理遅延の上限を抑える（実時間クロックは眠る: `Intake::Forwarded`）。
const IDLE_POLL_MAX: Duration = Duration::from_millis(2);
/// 実時間クロックで、再生終了待ち（`is_playing` はフラグのポーリング）がある間の起床間隔。
const AUDIO_POLL: Duration = Duration::from_millis(16);
/// 実時間クロックで、トランジション完了待ちがある間の安全網の間隔（通常は完了通知で起きる）。
const TRANS_POLL: Duration = Duration::from_millis(50);
/// 何も待っていなくても、未知の起床要因の取りこぼしがあっても止まらないための最長の眠り。
const MAX_SLEEP: Duration = Duration::from_millis(500);

/// 別スレッドから `drive` を起こす呼び鈴（立てたフラグは `wait` が消費するので、`wait` に入る前の `ring` も失われない）。
struct Doorbell {
    flag: std::sync::Mutex<bool>,
    cv: std::sync::Condvar,
}

impl Doorbell {
    fn new() -> Self {
        Self {
            flag: std::sync::Mutex::new(false),
            cv: std::sync::Condvar::new(),
        }
    }

    fn ring(&self) {
        *self.flag.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.cv.notify_one();
    }

    fn wait(&self, timeout: Duration) {
        let mut flag = self.flag.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !*flag {
            flag = self
                .cv
                .wait_timeout(flag, timeout)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        *flag = false;
    }
}

#[derive(Default)]
struct ForwardedInput {
    events: VecDeque<InputEvent>,
    closed: bool,
}

/// `drive` の入力の受け口。
enum Intake {
    /// ヘッドレス: 従来どおり `drive` が毎周 `try_recv` する（仮想クロックの決定論を保つため）。
    Direct(UnboundedReceiver<InputEvent>),
    /// 実時間: 入力チャンネルとトランジション完了通知を別スレッドで待ち、`Doorbell` で `drive` を起こす。
    Forwarded(ForwardedIntake),
}

struct ForwardedIntake {
    shared: Arc<std::sync::Mutex<ForwardedInput>>,
    bell: Arc<Doorbell>,
    stop: Arc<tokio::sync::Notify>,
    handle: Option<JoinHandle<()>>,
}

impl Intake {
    fn forwarded(mut input_rx: UnboundedReceiver<InputEvent>, trans_notify: Arc<tokio::sync::Notify>) -> Self {
        let shared = Arc::new(std::sync::Mutex::new(ForwardedInput::default()));
        let bell = Arc::new(Doorbell::new());
        let stop = Arc::new(tokio::sync::Notify::new());
        let thread_ctx = (shared.clone(), bell.clone(), stop.clone());
        let handle = std::thread::Builder::new()
            .name("noiria-tsint-wake".into())
            .spawn(move || {
                let (shared, bell, stop) = thread_ctx;
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().build() else {
                    shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner).closed = true;
                    bell.ring();
                    return;
                };
                rt.block_on(async {
                    let mut input_open = true;
                    loop {
                        tokio::select! {
                            ev = input_rx.recv(), if input_open => {
                                let mut s = shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                                match ev {
                                    Some(ev) => s.events.push_back(ev),
                                    None => {
                                        s.closed = true;
                                        input_open = false;
                                    }
                                }
                                drop(s);
                                bell.ring();
                            }
                            () = trans_notify.notified() => bell.ring(),
                            () = stop.notified() => break,
                        }
                    }
                });
            })
            .ok();
        Intake::Forwarded(ForwardedIntake {
            shared,
            bell,
            stop,
            handle,
        })
    }

    /// 届いている入力を `pending.input_buffer` へ移す。
    fn drain(&mut self, pending: &Rc<RefCell<PendingOps>>) {
        match self {
            Intake::Direct(rx) => loop {
                match rx.try_recv() {
                    Ok(ev) => pending.borrow_mut().input_buffer.push_back(ev),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        pending.borrow_mut().input_closed = true;
                        break;
                    }
                }
            },
            Intake::Forwarded(f) => {
                let mut s = f.shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut p = pending.borrow_mut();
                p.input_buffer.append(&mut s.events);
                if s.closed {
                    p.input_closed = true;
                }
            }
        }
    }

    /// 次に起きるまで眠る。`Direct` は従来どおりの短いポーリング、`Forwarded` は期限か呼び鈴まで。
    fn sleep(&self, timeout: Option<Duration>) {
        match self {
            Intake::Direct(_) => {
                let t = timeout.map_or(IDLE_POLL_MAX, |t| t.min(IDLE_POLL_MAX));
                std::thread::sleep(t.max(Duration::from_millis(1)));
            }
            Intake::Forwarded(f) => f.bell.wait(timeout.unwrap_or(MAX_SLEEP).min(MAX_SLEEP)),
        }
    }
}

impl Drop for ForwardedIntake {
    fn drop(&mut self) {
        self.stop.notify_one();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// 非同期 ops を駆動するイベントループ。V8 版の `js.run_event_loop().await` に相当する。
fn drive(
    interp: &mut Interpreter,
    es: &Rc<RefCell<EngineState>>,
    pending: &Rc<RefCell<PendingOps>>,
    mut intake: Intake,
    wakes: Option<&AtomicU64>,
) -> Result<(), String> {
    loop {
        if let Some(w) = wakes {
            w.fetch_add(1, AtomicOrdering::Relaxed);
        }
        // 1) 入力を取り込む。
        intake.drain(pending);

        // ヘッドレスの tick ドレインバリア（`crate::bridge::TickSync` 参照）:
        // headless.rs 側の要求世代を、これから読む `now` より**先に**読む。
        // `requested_generation()` → `clock.now()` の順で読むことで、この後の
        // `now` が headless の対応する `Clock::advance()` を確実に観測できる
        // （`TickSync` ドキュメントの happens-before の説明を参照。順序を逆にすると
        // この保証が崩れるので変更しないこと）。
        let tick_req = es.borrow().tick_sync.requested_generation();
        // タイマー期限・オーディオ猶予は clock 基準（windowed の Clock::Real なら
        // Instant::now() と同値で挙動不変、ヘッドレスの Clock::Virtual なら仮想 tick
        // に対して決定的）。
        let now = es.borrow().clock.now();
        let mut resolved_any = false;

        // 2) タイマー (op_wait) の期限が来たものを解決する。
        let due: Vec<HostPromise> = {
            let mut p = pending.borrow_mut();
            // 期限が来たものが無い周（ほとんど）は一覧を作り直さない。
            if p.timers.iter().any(|(deadline, _)| *deadline <= now) {
                let (due, remaining): (Vec<_>, Vec<_>) = std::mem::take(&mut p.timers)
                    .into_iter()
                    .partition(|(deadline, _)| *deadline <= now);
                p.timers = remaining;
                due.into_iter().map(|(_, h)| h).collect()
            } else {
                Vec::new()
            }
        };
        for h in due {
            resolved_any = true;
            h.resolve(interp.vm(), Value::Undefined);
        }

        // 3) トランジション完了待ち (op_trans_wait) をポーリングする。
        // 待っている人がいないときはシーンのロックを取らない。
        if !pending.borrow().trans.is_empty() && !es.borrow().trans_active() {
            let waiters: Vec<HostPromise> = std::mem::take(&mut pending.borrow_mut().trans);
            for h in waiters {
                resolved_any = true;
                h.resolve(interp.vm(), Value::Undefined);
            }
        }

        // 4) オーディオ再生終了待ち (op_audio_wait_end) をポーリングする。
        if !pending.borrow().audio.is_empty() {
            let items = std::mem::take(&mut pending.borrow_mut().audio);
            let mut still = Vec::with_capacity(items.len());
            for (ch, h) in items {
                if es.borrow().audio_busy(ch) {
                    still.push((ch, h));
                } else {
                    resolved_any = true;
                    h.resolve(interp.vm(), Value::Undefined);
                }
            }
            pending.borrow_mut().audio = still;
        }

        // 5) 入力イベント (op_input_next_event) を配送する。
        {
            let handle = {
                let mut p = pending.borrow_mut();
                if p.input.is_some() && (!p.input_buffer.is_empty() || p.input_closed) {
                    p.input.take()
                } else {
                    None
                }
            };
            if let Some(h) = handle {
                let ev = pending.borrow_mut().input_buffer.pop_front();
                resolved_any = true;
                match ev {
                    Some(ev) => h.resolve(interp.vm(), input_event_to_value(&ev)),
                    None => h.resolve(interp.vm(), quit_event_value()),
                }
            }
        }

        // 6) マイクロタスクキュー・トップレベル await の継続を汲み尽くす。
        //    `tsint::vm::Vm::run_event_loop` はマイクロタスクキューを常に空になるまで
        //    汲み尽くしてから `PumpState::Idle`/`Waiting` を返す（`pending_host_count`
        //    の残数だけで判定。tsint リポジトリ `src/vm.rs` 参照）。`Waiting` は
        //    「まだ解決されていないホスト Promise（タイマー/オーディオ/入力待ち等）が
        //    残っている」という意味で、`op_input_next_event` が入力待ちハンドルを
        //    登録した直後は常にこれに該当する（そのハンドル自身が未解決の
        //    ホスト Promise としてカウントされるため）。よって「入力ポンプが次の
        //    イベント待ちに入った」瞬間は原理的に必ず `Waiting` になり、`Idle` には
        //    ならない。
        let pump = interp.run_event_loop();

        // 6.5) ヘッドレスのロックステップ同期: 上記の通りマイクロタスクは pump の
        //      戻り値（Idle/Waiting どちらでも）に関わらずこの時点で必ず汲み尽くされて
        //      いるので、`pump` の値では判定しない（`Idle` のみに限定すると、入力待ち
        //      登録直後は常に `Waiting` になるため ack が一切発火しなくなるバグになる）。
        //      入力ポンプが次のイベント待ちに入っている（保留ハンドルあり・バッファ空）
        //      なら、このハンドルに対して一度だけ ack を出す（V8 版 `run_script` の
        //      イベントループラッパと同じ同期点。`docs/implementation-notes.md`
        //      「2026-07-13 tsint tick ドレインバリア」参照）。windowed 実行では誰も
        //      観測しないため挙動不変。`pump` 自体は次段（7）の終了判定で使う。
        let should_ack = {
            let mut p = pending.borrow_mut();
            if p.input.is_some() && p.input_buffer.is_empty() && !p.input_acked {
                p.input_acked = true;
                true
            } else {
                false
            }
        };
        if should_ack {
            es.borrow().input_sync.ack();
        }

        // 7) 終了判定: 保留中のホスト Promise が無く、かつマイクロタスクも
        //    空なら、モジュールタスク（トップレベル await）が settled した
        //    とみなせる（V8 版で `run_script` の future が完了する条件と同義）。
        let all_empty = {
            let p = pending.borrow();
            p.timers.is_empty() && p.trans.is_empty() && p.audio.is_empty() && p.input.is_none()
        };
        if matches!(pump, PumpState::Idle) && all_empty {
            log::info!("tsint: スクリプトのイベントループが終了しました");
            return Ok(());
        }

        // 8) 次のティックまでの待機時間。次のタイマー/オーディオ猶予期限か、
        //    無ければ IDLE_POLL_MAX 上限でポーリングする（入力の反応性とビジーループ
        //    回避のバランス）。旧実装は 50ms 上限だったが、headless.rs のフレーム
        //    ループ（16ms 刻み）より粗いポーリング間隔だと、`--autotest` が送る
        //    move/click イベントの取りこぼし〜処理までの遅延がフレームより長くなり得て
        //    dist（tsint 専用）ビルドの `--autotest` フレーム差異の主要因の一つだった
        //    （`docs/implementation-notes.md`「2026-07-13 dist フレーム差異の調査と
        //    headless 決定論化」参照）。IDLE_POLL_MAX をフレーム刻みより十分小さくして
        //    この遅延の上限を縮める。
        if !resolved_any {
            // ヘッドレスの tick ドレインバリア: このループの先頭で読んだ `tick_req`
            // （`now` より先に読んでいるので `now` が対応する `Clock::advance()` を
            // 確実に観測している）の時点で処理すべきものは全て処理し切った
            // （timer/trans/audio/input のいずれも今回は何も解決しなかった）ので、
            // headless 側へ完了を報告する。windowed/V8 では誰も待たないので無害。
            es.borrow().tick_sync.mark_complete(tick_req);
            let sleep_for = {
                let p = pending.borrow();
                let now = es.borrow().clock.now();
                let next_timer = p
                    .timers
                    .iter()
                    .map(|(d, _)| d.saturating_duration_since(now))
                    .min();
                // 再生終了・トランジション完了はフラグを見に行く方式なので、待っている間だけ短い間隔で起きる
                // （トランジション完了は通常、完了通知で起こされる）。
                let audio = (!p.audio.is_empty()).then_some(AUDIO_POLL);
                let trans = (!p.trans.is_empty()).then_some(TRANS_POLL);
                [next_timer, audio, trans].into_iter().flatten().min()
            };
            intake.sleep(sleep_for);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crossbeam_channel::unbounded;
    use noiria_core::assets::GameDir;
    use noiria_core::audio::AudioShared;
    use noiria_core::scene::Scene;
    use tokio::sync::Notify;

    use crate::bridge::{SharedScene, WakeSink};

    #[test]
    fn normalize_game_rel_resolves_relative_and_clamps_at_root() {
        assert_eq!(normalize_game_rel("", "main.ts"), "main.ts");
        assert_eq!(
            normalize_game_rel("sub/main.ts", "./util.ts"),
            "sub/util.ts"
        );
        assert_eq!(normalize_game_rel("a/b/c.ts", "../d.ts"), "a/d.ts");
        // ルートより上へは脱出できない（過剰な `..` は単に無視される）。
        assert_eq!(
            normalize_game_rel("a.ts", "../../../etc/passwd"),
            "etc/passwd"
        );
    }

    #[test]
    fn tsint_can_parse_all_embedded_runtime_modules() {
        // V8 側 (`loader.rs::runtime_modules_all_transpile`) の tsint 版。
        // deno_ast の transpile ではなく tsint 自身のパーサで構文エラーが
        // 無いことを確認する（型注記/interface 等は読み飛ばされる想定）。
        for (name, src) in RUNTIME_MODULES {
            let result = tsint::parse_module(src, name);
            assert!(result.is_ok(), "{name} のパースに失敗: {:?}", result.err());
        }
    }

    /// テスト専用の使い捨てディレクトリ（drop 時に自動削除）。
    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let id = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("{tag}-{id}"));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup_game_dir(main_ts: &str) -> (GameDir, noiria_core::assets::GameConfig, TempDir) {
        let dir = TempDir::new("noiria-tsint-test");
        std::fs::write(
            dir.path().join("game.json"),
            r#"{"title":"t","id":"noiria-tsint-test","main":"main.ts"}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("main.ts"), main_ts).unwrap();
        let (game_dir, config) = GameDir::open(dir.path()).unwrap();
        (game_dir, config, dir)
    }

    /// `spawn`/`run_script` と同じ配線を、実スレッドを起こさずテストスレッド上で
    /// 直接駆動する end-to-end テスト。ops の型変換（Value <-> Rust 構造体）が
    /// 一通り正しく配線されていることを検証する（V8 側 kag_full.rs 相当の
    /// tsint 実 ops 版）。
    #[test]
    fn tsint_ops_wiring_runs_a_small_script_to_completion() {
        let main_ts = r#"
const ops = (globalThis as any).Deno.core.ops;
const id: number = ops.op_layer_new("image");
ops.op_layer_set_props(id, "fore", { x: 10, y: 20, z: 1, opacity: 200, visible: true });
ops.op_log("info", "hello from tsint");
const t0 = ops.op_now();
if (typeof t0 !== "number") throw new Error("op_now should return a number");
await ops.op_wait(5);
ops.op_quit();
"#;
        let (game_dir, config, _guard) = setup_game_dir(main_ts);
        let scene: SharedScene = Arc::new(Mutex::new(Scene::new(64, 64)));
        let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
        drop(input_tx); // 入力は使わない: チャンネルを即座に閉じておく
        let (user_tx, user_rx) = unbounded();
        let (audio_tx, audio_rx) = unbounded();
        drop(audio_rx);

        let ctx = EngineCtx {
            scene: scene.clone(),
            proxy: WakeSink::Channel(user_tx),
            input_rx,
            trans_notify: Arc::new(Notify::new()),
            audio_tx,
            audio_shared: Arc::new(AudioShared::default()),
            game_dir,
            config,
            start: Instant::now(),
            clock: Clock::Real,
            input_sync: InputSync::new(),
            tick_sync: TickSync::new(),
            pending_waits: PendingWaits::new(),
            op_log: None,
        };

        run_script(ctx).expect("tsint スクリプトの実行に失敗");

        // op_layer_set_props が実際にシーンへ反映されているか確認する。
        let scene = scene.lock().unwrap();
        let layer = scene
            .layer(noiria_core::scene::Page::Fore, 1)
            .expect("レイヤが作られていない");
        assert_eq!(
            (layer.x, layer.y, layer.z, layer.opacity, layer.visible),
            (10, 20, 1, 200, true)
        );
        drop(scene);

        // op_quit -> Wake::quit() が呼ばれ、Quit イベントが飛んでいるはず。
        let mut saw_quit = false;
        while let Ok(ev) = user_rx.try_recv() {
            if matches!(ev, UserEvent::Quit) {
                saw_quit = true;
            }
        }
        assert!(saw_quit, "op_quit が WakeSink へ Quit を送っていない");
    }

    /// move 系 ops (`op_layer_move`/`op_layer_moving`) の tsint 版配線テスト
    /// （V8 側 `script::js_tests::move_tween_reaches_to_value_in_v8` 相当）。
    #[test]
    fn tsint_move_tween_reaches_to_value() {
        let main_ts = r#"
const ops = (globalThis as any).Deno.core.ops;
const id: number = ops.op_layer_new("image");
ops.op_layer_set_props(id, "fore", { x: 0, y: 0, opacity: 0, visible: true });
ops.op_layer_move(id, "fore", { toX: 100, toOpacity: 255, timeMs: 40 });
await ops.op_wait(80);
if (ops.op_layer_moving()) throw new Error("moving は tween の duration 経過後は false のはず");
ops.op_quit();
"#;
        let (game_dir, config, _guard) = setup_game_dir(main_ts);
        let scene: SharedScene = Arc::new(Mutex::new(Scene::new(64, 64)));
        let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
        drop(input_tx);
        let (user_tx, _user_rx) = unbounded();
        let (audio_tx, audio_rx) = unbounded();
        drop(audio_rx);

        let ctx = EngineCtx {
            scene: scene.clone(),
            proxy: WakeSink::Channel(user_tx),
            input_rx,
            trans_notify: Arc::new(Notify::new()),
            audio_tx,
            audio_shared: Arc::new(AudioShared::default()),
            game_dir,
            config,
            start: Instant::now(),
            clock: Clock::Real,
            input_sync: InputSync::new(),
            tick_sync: TickSync::new(),
            pending_waits: PendingWaits::new(),
            op_log: None,
        };

        run_script(ctx).expect("tsint スクリプトの実行に失敗");

        // 完了した tween の確定は renderer の責務: 1 周期ぶんを模して呼ぶ。
        let mut scene = scene.lock().unwrap();
        let now = Instant::now();
        scene.settle_finished_tweens(now);
        let layer = scene
            .layer(noiria_core::scene::Page::Fore, 1)
            .expect("レイヤが作られていない");
        assert!(layer.tween.is_none(), "settle 後は tween が外れているはず");
        assert_eq!(
            (layer.x, layer.opacity),
            (100, 255),
            "位置/不透明度が to 値になっているはず"
        );
    }

    /// [move path=]（W5）の tsint 版配線テスト: `op_layer_move_path` へ 2 区間の配列を
    /// 渡し、第一区間の tween 開始 + 残り区間の tween_queue 積みが V8 側と同じ Value 変換で
    /// 動くことを確認する（区間をまたぐ継ぎ目自体は noiria-core の scene.rs 側で検証済み）。
    #[test]
    fn tsint_move_path_wiring_starts_first_leg_and_queues_rest() {
        let main_ts = r#"
const ops = (globalThis as any).Deno.core.ops;
const id: number = ops.op_layer_new("image");
ops.op_layer_set_props(id, "fore", { x: 0, y: 0, opacity: 0, visible: true });
ops.op_layer_move_path(id, "fore", [
  { toX: 50, timeMs: 40 },
  { toX: 150, toY: 80, toOpacity: 128, timeMs: 40 },
], false);
ops.op_quit();
"#;
        let (game_dir, config, _guard) = setup_game_dir(main_ts);
        let scene: SharedScene = Arc::new(Mutex::new(Scene::new(64, 64)));
        let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
        drop(input_tx);
        let (user_tx, _user_rx) = unbounded();
        let (audio_tx, audio_rx) = unbounded();
        drop(audio_rx);

        let ctx = EngineCtx {
            scene: scene.clone(),
            proxy: WakeSink::Channel(user_tx),
            input_rx,
            trans_notify: Arc::new(Notify::new()),
            audio_tx,
            audio_shared: Arc::new(AudioShared::default()),
            game_dir,
            config,
            start: Instant::now(),
            clock: Clock::Real,
            input_sync: InputSync::new(),
            tick_sync: TickSync::new(),
            pending_waits: PendingWaits::new(),
            op_log: None,
        };

        run_script(ctx).expect("tsint スクリプトの実行に失敗");

        let scene = scene.lock().unwrap();
        let layer = scene
            .layer(noiria_core::scene::Page::Fore, 1)
            .expect("レイヤが作られていない");
        let t = layer
            .tween
            .as_ref()
            .expect("第一区間の tween が開始しているはず");
        // main_ts で opacity: 0 に設定済みなので、toOpacity 省略時はその現在値 0 を引き継ぐ。
        assert_eq!(
            (t.to_x, t.to_y, t.to_opacity),
            (50, 0, 0),
            "第一区間: y/opacity は現在値を引き継ぐ"
        );
        assert!(t.spline.is_none());
        assert_eq!(layer.tween_queue.len(), 1);
        let wp = &layer.tween_queue[0];
        assert_eq!((wp.x, wp.y, wp.opacity), (150, 80, 128));
        assert!(wp.spline.is_none());
    }

    /// drive の待機・起床の測定用: 実時間クロックで 1 本のスクリプトを別スレッドで走らせる。
    struct DriveRun {
        input_tx: tokio::sync::mpsc::UnboundedSender<InputEvent>,
        wakes: Arc<AtomicU64>,
        finished: Arc<std::sync::atomic::AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
        _dir: TempDir,
    }

    impl DriveRun {
        fn start(main_ts: &str) -> Self {
            let (game_dir, config, dir) = setup_game_dir(main_ts);
            let scene: SharedScene = Arc::new(Mutex::new(Scene::new(64, 64)));
            let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
            let (user_tx, _user_rx) = unbounded();
            let (audio_tx, audio_rx) = unbounded();
            drop(audio_rx);
            let ctx = EngineCtx {
                scene,
                proxy: WakeSink::Channel(user_tx),
                input_rx,
                trans_notify: Arc::new(Notify::new()),
                audio_tx,
                audio_shared: Arc::new(AudioShared::default()),
                game_dir,
                config,
                start: Instant::now(),
                clock: Clock::Real,
                input_sync: InputSync::new(),
                tick_sync: TickSync::new(),
                pending_waits: PendingWaits::new(),
                op_log: None,
            };
            let wakes = Arc::new(AtomicU64::new(0));
            let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (w, f) = (wakes.clone(), finished.clone());
            let handle = std::thread::spawn(move || {
                run_script_counted(ctx, Some(&w)).expect("tsint スクリプトの実行に失敗");
                f.store(true, AtomicOrdering::SeqCst);
            });
            Self {
                input_tx,
                wakes,
                finished,
                handle: Some(handle),
                _dir: dir,
            }
        }

        fn wakes(&self) -> u64 {
            self.wakes.load(AtomicOrdering::Relaxed)
        }

        /// 終わるまで待つ。`limit` を超えたら panic。終わるまでの経過時間を返す。
        fn join_within(&mut self, limit: Duration) -> Duration {
            let t = Instant::now();
            while !self.finished.load(AtomicOrdering::SeqCst) {
                assert!(t.elapsed() < limit, "スクリプトが {limit:?} 以内に終わらない");
                std::thread::sleep(Duration::from_millis(1));
            }
            self.handle.take().unwrap().join().unwrap();
            t.elapsed()
        }
    }

    fn click() -> InputEvent {
        InputEvent {
            kind: "click",
            x: 0.0,
            y: 0.0,
            button: Some("left"),
            key: None,
            delta: None,
            t: 0.0,
        }
    }

    /// 本番（windowed）の `timeBeginPeriod(1)` 下を模す（待機中の起床が 1ms 刻みになる条件）。
    #[cfg(windows)]
    fn begin_1ms_timer_period() {
        #[link(name = "winmm")]
        extern "system" {
            fn timeBeginPeriod(period: u32) -> u32;
        }
        // SAFETY: 引数は有効な分解能（ms）。プロセス内で有効なだけで状態を壊さない。
        unsafe {
            timeBeginPeriod(1);
        }
    }
    #[cfg(not(windows))]
    fn begin_1ms_timer_period() {}

    /// このプロセスの CPU 時間（カーネル + ユーザー、ms）。待機中の CPU 消費の測定用。
    #[cfg(windows)]
    fn process_cpu_ms() -> f64 {
        #[repr(C)]
        #[derive(Default, Clone, Copy)]
        struct FileTime {
            lo: u32,
            hi: u32,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> isize;
            fn GetProcessTimes(h: isize, c: *mut FileTime, e: *mut FileTime, k: *mut FileTime, u: *mut FileTime) -> i32;
        }
        let (mut c, mut e, mut k, mut u) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
        // SAFETY: 4 つとも有効な書き込み先。現在プロセスのハンドルは疑似ハンドルで閉じる必要がない。
        unsafe {
            GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        }
        let t = |f: FileTime| ((u64::from(f.hi) << 32) | u64::from(f.lo)) as f64 / 1e4;
        t(k) + t(u)
    }
    #[cfg(not(windows))]
    fn process_cpu_ms() -> f64 {
        0.0
    }

    const WAIT_INPUT_THEN_QUIT: &str = r#"
const ops = (globalThis as any).Deno.core.ops;
await ops.op_input_next_event();
ops.op_quit();
"#;

    /// クリック待ちの間、drive が 1 秒に何回起きるか（旧実装は 1〜2ms スリープで永久に再ポーリング）。
    #[test]
    fn drive_sleeps_while_waiting_for_input() {
        begin_1ms_timer_period();
        let mut run = DriveRun::start(WAIT_INPUT_THEN_QUIT);
        std::thread::sleep(Duration::from_millis(1500)); // 起動（モジュール評価）を済ませる
        let before = run.wakes();
        let cpu_before = process_cpu_ms();
        std::thread::sleep(Duration::from_millis(1000));
        let idle_wakes = run.wakes() - before;
        eprintln!("DRIVE_IDLE_WAKES_PER_SEC={idle_wakes} DRIVE_IDLE_CPU_MS_PER_SEC={:.1}", process_cpu_ms() - cpu_before);
        let sent = Instant::now();
        run.input_tx.send(click()).unwrap();
        let latency = run.join_within(Duration::from_secs(2));
        eprintln!("DRIVE_INPUT_LATENCY_MS={}", latency.as_millis());
        assert!(sent.elapsed() < Duration::from_secs(2));
        assert!(idle_wakes <= 10, "入力待ちで 1 秒に {idle_wakes} 回起きている（眠っていない）");
        assert!(latency < Duration::from_millis(100), "入力から再開まで {latency:?}");
    }
}
