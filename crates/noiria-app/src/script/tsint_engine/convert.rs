//! tsint の `Value` と Rust の型・DTO 間の相互変換ヘルパー。

use std::rc::Rc;

use noiria_core::savedata::SlotInfo;
use noiria_core::text::{
    GlyphRect, LocateArgs, TextAlign, TextSegment, TextStyle, MAX_OUTLINE_WIDTH,
};
use tsint::value::Value;
use tsint::vm::{number_to_js_string, to_number, to_string_basic, Vm};

use crate::bridge::InputEvent;
use crate::script::engine_state::{
    BlitClip, ImageLoadOpts, LayerProps, MoveArgs, PrintResult, TransArgs,
};
use crate::script::numconv::{to_coord, to_i32, to_time_ms, to_u32, to_u8};

const UNDEF: &Value = &Value::Undefined;

pub(super) fn arg(args: &[Value], i: usize) -> &Value {
    args.get(i).unwrap_or(UNDEF)
}

pub(super) fn as_string(v: &Value) -> String {
    match v {
        Value::Str(s) => s.to_string(),
        Value::Undefined | Value::Null => String::new(),
        other => to_string_basic(other),
    }
}

pub(super) fn num(v: &Value) -> f64 {
    to_number(v)
}

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

fn opt_prop<T>(
    vm: &mut Vm,
    obj: &Value,
    key: &str,
    f: fn(&Value) -> T,
) -> Result<Option<T>, Value> {
    let v = vm.get_prop(obj, key)?;
    Ok((!v.is_nullish()).then(|| f(&v)))
}

fn as_f32(v: &Value) -> f32 {
    num(v) as f32
}

pub(super) fn rgba_from_value(v: &Value) -> [u8; 4] {
    let items = tsint::vm::array_values(v);
    std::array::from_fn(|i| items.get(i).map_or(0, as_u8))
}

pub(super) fn layer_props_from_value(vm: &mut Vm, v: &Value) -> Result<LayerProps, Value> {
    Ok(LayerProps {
        x: opt_prop(vm, v, "x", as_coord)?,
        y: opt_prop(vm, v, "y", as_coord)?,
        z: opt_prop(vm, v, "z", as_i32)?,
        opacity: opt_prop(vm, v, "opacity", as_u8)?,
        visible: opt_prop(vm, v, "visible", as_bool)?,
    })
}

pub(super) fn text_style_from_value(vm: &mut Vm, v: &Value) -> Result<TextStyle, Value> {
    let mut style = TextStyle::default();
    if let Some(ff) = opt_prop(vm, v, "fontFamily", as_string)? {
        style.font_family = ff;
    }
    if let Some(fs) = opt_prop(vm, v, "fontSize", as_f32)? {
        style.font_size = fs;
    }
    if let Some(lh) = opt_prop(vm, v, "lineHeight", as_f32)? {
        if !lh.is_finite() {
            return Err(vm.type_error("lineHeight must be a finite f32"));
        }
        style.line_height = lh;
    }
    if let Some(c) = opt_prop(vm, v, "color", rgba_from_value)? {
        style.color = c;
    }
    if let Some(oc) = opt_prop(vm, v, "outlineColor", rgba_from_value)? {
        style.outline_color = oc;
    }
    if let Some(ow) = opt_prop(vm, v, "outlineWidth", as_u32)? {
        style.outline_width = ow.min(MAX_OUTLINE_WIDTH);
    }
    if let Some(bw) = opt_prop(vm, v, "boxW", as_u32)? {
        style.box_w = bw;
    }
    if let Some(bh) = opt_prop(vm, v, "boxH", as_u32)? {
        style.box_h = bh;
    }
    if let Some(mx) = opt_prop(vm, v, "marginX", as_i32)? {
        style.margin_x = mx;
    }
    if let Some(my) = opt_prop(vm, v, "marginY", as_i32)? {
        style.margin_y = my;
    }
    if let Some(align) = opt_prop(vm, v, "align", as_string)? {
        style.align = match align.as_str() {
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
        let text = as_string(&vm.get_prop(&item, "text")?);
        let color = opt_prop(vm, &item, "color", rgba_from_value)?;
        let size = opt_prop(vm, &item, "size", as_f32)?;
        let bold = opt_prop(vm, &item, "bold", as_bool)?;
        let edge_color = opt_prop(vm, &item, "edgeColor", rgba_from_value)?;
        let ruby = opt_prop(vm, &item, "ruby", as_string)?;
        let locate_v = vm.get_prop(&item, "locate")?;
        let locate = if locate_v.is_nullish() {
            None
        } else {
            Some(LocateArgs {
                x: as_f32(&vm.get_prop(&locate_v, "x")?),
                y: opt_prop(vm, &locate_v, "y", as_f32)?,
            })
        };
        let indent = opt_prop(vm, &item, "indent", as_bool)?;
        let inline_image = opt_prop(vm, &item, "inlineImage", as_string)?;
        let font_family = opt_prop(vm, &item, "fontFamily", as_string)?;
        out.push(TextSegment {
            text,
            color,
            size,
            bold,
            edge_color,
            ruby,
            locate,
            indent,
            inline_image,
            font_family,
        });
    }
    Ok(out)
}

pub(super) fn move_args_from_value(vm: &mut Vm, v: &Value) -> Result<MoveArgs, Value> {
    Ok(MoveArgs {
        to_x: opt_prop(vm, v, "toX", as_coord)?,
        to_y: opt_prop(vm, v, "toY", as_coord)?,
        to_opacity: opt_prop(vm, v, "toOpacity", as_u8)?,
        time_ms: opt_prop(vm, v, "timeMs", |v| to_time_ms(num(v)))?.unwrap_or(0),
        accel: opt_prop(vm, v, "accel", as_f32)?.unwrap_or(0.0),
    })
}

/// `layer_blit_image`（W4）の clip= 引数: `{x,y,w,h}` オブジェクトまたは
/// undefined/null（画像全体）。
pub(super) fn blit_clip_from_value(vm: &mut Vm, v: &Value) -> Result<Option<BlitClip>, Value> {
    if v.is_nullish() {
        return Ok(None);
    }
    Ok(Some(BlitClip {
        x: as_u32(&vm.get_prop(v, "x")?),
        y: as_u32(&vm.get_prop(v, "y")?),
        w: as_u32(&vm.get_prop(v, "w")?),
        h: as_u32(&vm.get_prop(v, "h")?),
    }))
}

pub(super) fn trans_args_from_value(vm: &mut Vm, v: &Value) -> Result<TransArgs, Value> {
    Ok(TransArgs {
        method: as_string(&vm.get_prop(v, "method")?),
        time_ms: to_time_ms(num(&vm.get_prop(v, "timeMs")?)),
        rule: opt_prop(vm, v, "rule", as_string)?,
        vague: opt_prop(vm, v, "vague", as_f32)?,
        from: opt_prop(vm, v, "from", as_string)?,
        stay: opt_prop(vm, v, "stay", as_string)?,
    })
}

/// `layer_move_path`（W5）の points= 引数: 各要素が `move_args_from_value` と同じ形の配列。
pub(super) fn move_args_array_from_value(vm: &mut Vm, v: &Value) -> Result<Vec<MoveArgs>, Value> {
    let arr = tsint::vm::array_values(v);
    arr.iter()
        .map(|item| move_args_from_value(vm, item))
        .collect()
}

/// [image]（W5 拡張）の opts= 引数: `{clip, flipH, flipV, grayscale}`。
pub(super) fn image_load_opts_from_value(vm: &mut Vm, v: &Value) -> Result<ImageLoadOpts, Value> {
    let clip_v = vm.get_prop(v, "clip")?;
    Ok(ImageLoadOpts {
        clip: blit_clip_from_value(vm, &clip_v)?,
        flip_h: as_bool(&vm.get_prop(v, "flipH")?),
        flip_v: as_bool(&vm.get_prop(v, "flipV")?),
        grayscale: as_bool(&vm.get_prop(v, "grayscale")?),
    })
}

pub(super) fn obj_from_pairs<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    let o = Value::new_plain_object();
    if let Value::Object(rc) = &o {
        let mut b = rc.borrow_mut();
        for (k, v) in pairs {
            b.props.set_value(Rc::from(k), v);
        }
    }
    o
}

pub(super) fn print_result_to_value(r: PrintResult) -> Value {
    obj_from_pairs([
        ("total", Value::Number(r.total as f64)),
        ("added", Value::Number(r.added as f64)),
    ])
}

pub(super) fn glyph_rects_to_value(rects: Vec<GlyphRect>) -> Value {
    Value::new_array(
        rects
            .into_iter()
            .map(|g| {
                obj_from_pairs([
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
            .map(|s| obj_from_pairs([("slot", Value::str(s.slot)), ("json", Value::str(s.json))]))
            .collect(),
    )
}

pub(super) fn input_event_to_value(ev: &InputEvent) -> Value {
    obj_from_pairs([
        ("kind", Value::str(ev.kind)),
        ("x", Value::Number(ev.x)),
        ("y", Value::Number(ev.y)),
        ("t", Value::Number(ev.t)),
        ("button", ev.button.map_or(Value::Undefined, Value::str)),
        (
            "key",
            ev.key.as_deref().map_or(Value::Undefined, Value::str),
        ),
        ("delta", ev.delta.map_or(Value::Undefined, Value::Number)),
    ])
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

/// 未捕捉の Promise reject を大まかに文字列化する（ログ用）。
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
