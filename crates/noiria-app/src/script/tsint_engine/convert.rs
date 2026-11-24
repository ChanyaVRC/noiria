//! tsint の `Value` と Rust の型・DTO 間の相互変換ヘルパー。

use std::rc::Rc;

use noiria_core::savedata::SlotInfo;
use noiria_core::text::{GlyphRect, LocateArgs, TextAlign, TextSegment, TextStyle};
use tsint::value::Value;
use tsint::vm::{number_to_js_string, to_number, to_string_basic, Vm};

use crate::bridge::InputEvent;
use crate::script::engine_state::{
    BlitClip, ImageLoadOpts, LayerProps, MoveArgs, PrintResult, TransArgs,
};
use crate::script::numconv::{to_coord, to_i32, to_time_ms, to_u32, to_u8};

// =====================================================================
// Value 変換ヘルパ。V8 側の #[serde]/プリミティブ引数変換に相当する。
// =====================================================================

pub(super) fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

pub(super) fn as_string(v: &Value) -> String {
    match v {
        Value::Str(s) => s.to_string(),
        Value::Undefined | Value::Null => String::new(),
        other => to_string_basic(other),
    }
}

pub(super) fn opt_string(v: &Value) -> Option<String> {
    if v.is_nullish() {
        None
    } else {
        Some(as_string(v))
    }
}

pub(super) fn num(v: &Value) -> f64 {
    to_number(v)
}

/// 整数への変換は両エンジン共通の規則（`numconv`）。
pub(super) fn as_u8(v: &Value) -> u8 {
    to_u8(num(v))
}

pub(super) fn as_u32(v: &Value) -> u32 {
    to_u32(num(v))
}

pub(super) fn as_i32(v: &Value) -> i32 {
    to_i32(num(v))
}

pub(super) fn as_coord(v: &Value) -> i32 {
    to_coord(num(v))
}

pub(super) fn as_bool(v: &Value) -> bool {
    v.to_bool()
}

pub(super) fn rgba_from_value(v: &Value) -> [u8; 4] {
    let items = tsint::vm::array_values(v);
    let get = |i: usize| items.get(i).cloned().unwrap_or(Value::Number(0.0));
    [
        as_u8(&get(0)),
        as_u8(&get(1)),
        as_u8(&get(2)),
        as_u8(&get(3)),
    ]
}

pub(super) fn layer_props_from_value(vm: &mut Vm, v: &Value) -> Result<LayerProps, Value> {
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

pub(super) fn text_style_from_value(vm: &mut Vm, v: &Value) -> Result<TextStyle, Value> {
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

pub(super) fn segments_from_value(vm: &mut Vm, v: &Value) -> Result<Vec<TextSegment>, Value> {
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

pub(super) fn move_args_from_value(vm: &mut Vm, v: &Value) -> Result<MoveArgs, Value> {
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
pub(super) fn blit_clip_from_value(vm: &mut Vm, v: &Value) -> Result<Option<BlitClip>, Value> {
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

pub(super) fn trans_args_from_value(vm: &mut Vm, v: &Value) -> Result<TransArgs, Value> {
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
pub(super) fn move_args_array_from_value(vm: &mut Vm, v: &Value) -> Result<Vec<MoveArgs>, Value> {
    let arr = tsint::vm::array_values(v);
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(move_args_from_value(vm, &item)?);
    }
    Ok(out)
}

/// [image]（W5 拡張）の opts= 引数: `{clip, flipH, flipV, grayscale}`。
pub(super) fn image_load_opts_from_value(vm: &mut Vm, v: &Value) -> Result<ImageLoadOpts, Value> {
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

pub(super) fn obj_from_pairs(pairs: &[(&str, Value)]) -> Value {
    let o = Value::new_plain_object();
    if let Value::Object(rc) = &o {
        let mut b = rc.borrow_mut();
        for (k, v) in pairs {
            b.props.set_value(Rc::from(*k), v.clone());
        }
    }
    o
}

pub(super) fn print_result_to_value(r: PrintResult) -> Value {
    obj_from_pairs(&[
        ("total", Value::Number(r.total as f64)),
        ("added", Value::Number(r.added as f64)),
    ])
}

pub(super) fn glyph_rects_to_value(rects: Vec<GlyphRect>) -> Value {
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

pub(super) fn slot_list_to_value(list: Vec<SlotInfo>) -> Value {
    Value::new_array(
        list.into_iter()
            .map(|s| obj_from_pairs(&[("slot", Value::str(s.slot)), ("json", Value::str(s.json))]))
            .collect(),
    )
}

pub(super) fn input_event_to_value(ev: &InputEvent) -> Value {
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

pub(super) fn quit_event_value() -> Value {
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
pub(super) fn debug_value(v: &Value) -> String {
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
