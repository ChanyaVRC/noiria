//! `globalThis.Deno.core.ops` へ登録する tsint ネイティブ ops 群。

use std::cell::RefCell;
use std::rc::Rc;

use noiria_core::audio::channel_index;
use tsint::value::Value;
use tsint::vm::{HostPromise, Vm};
use tsint::Interpreter;

use crate::script::engine_state::{video_optional_frame, EngineState};
use crate::script::numconv::{to_time_ms, wait_duration};

use super::convert::*;
use super::event_loop::PendingOps;


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

/// ops をインストールし、`globalThis.Deno.core.ops` へ生やす。
pub(super) fn install_ops(
    interp: &mut Interpreter,
    es: &Rc<RefCell<EngineState>>,
    pending: &Rc<RefCell<PendingOps>>,
) {
    let ops_obj = Value::new_plain_object();
    install_layer_ops(&ops_obj, es);
    install_text_ops(&ops_obj, es);
    install_trans_ops(&ops_obj, es, pending);
    install_audio_ops(&ops_obj, es, pending);
    install_video_ops(&ops_obj, es);
    install_input_ops(&ops_obj, pending);
    install_sys_ops(&ops_obj, es, pending);

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


fn install_layer_ops(ops_obj: &Value, es: &Rc<RefCell<EngineState>>) {
    macro_rules! op {
        ($name:literal, $value:expr) => {
            if let Value::Object(o) = ops_obj {
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

}

fn install_text_ops(ops_obj: &Value, es: &Rc<RefCell<EngineState>>) {
    macro_rules! op {
        ($name:literal, $value:expr) => {
            if let Value::Object(o) = ops_obj {
                o.borrow_mut().props.set_value(Rc::from($name), $value);
            }
        };
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

}

fn install_trans_ops(ops_obj: &Value, es: &Rc<RefCell<EngineState>>, pending: &Rc<RefCell<PendingOps>>) {
    macro_rules! op {
        ($name:literal, $value:expr) => {
            if let Value::Object(o) = ops_obj {
                o.borrow_mut().props.set_value(Rc::from($name), $value);
            }
        };
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

}

fn install_audio_ops(ops_obj: &Value, es: &Rc<RefCell<EngineState>>, pending: &Rc<RefCell<PendingOps>>) {
    macro_rules! op {
        ($name:literal, $value:expr) => {
            if let Value::Object(o) = ops_obj {
                o.borrow_mut().props.set_value(Rc::from($name), $value);
            }
        };
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

}

fn install_video_ops(ops_obj: &Value, es: &Rc<RefCell<EngineState>>) {
    macro_rules! op {
        ($name:literal, $value:expr) => {
            if let Value::Object(o) = ops_obj {
                o.borrow_mut().props.set_value(Rc::from($name), $value);
            }
        };
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

}

fn install_input_ops(ops_obj: &Value, pending: &Rc<RefCell<PendingOps>>) {
    macro_rules! op {
        ($name:literal, $value:expr) => {
            if let Value::Object(o) = ops_obj {
                o.borrow_mut().props.set_value(Rc::from($name), $value);
            }
        };
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

}

fn install_sys_ops(ops_obj: &Value, es: &Rc<RefCell<EngineState>>, pending: &Rc<RefCell<PendingOps>>) {
    macro_rules! op {
        ($name:literal, $value:expr) => {
            if let Value::Object(o) = ops_obj {
                o.borrow_mut().props.set_value(Rc::from($name), $value);
            }
        };
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
}

