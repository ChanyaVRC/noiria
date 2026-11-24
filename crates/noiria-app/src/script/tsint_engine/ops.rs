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

struct OpRegistrar<'a> {
    ops_obj: &'a Value,
    es: &'a Rc<RefCell<EngineState>>,
    pending: &'a Rc<RefCell<PendingOps>>,
}

impl<'a> OpRegistrar<'a> {
    fn set(&self, name: &'static str, value: Value) {
        if let Value::Object(o) = self.ops_obj {
            o.borrow_mut().props.set_value(Rc::from(name), value);
        }
    }

    fn es(
        &self,
        name: &'static str,
        arity: usize,
        f: impl Fn(&EngineState, &mut Vm, &[Value]) -> Result<Value, Value> + 'static,
    ) {
        let es = Rc::clone(self.es);
        self.set(
            name,
            sync_native(name, arity, move |vm, _this, args| f(&es.borrow(), vm, args)),
        );
    }

    fn es_mut(
        &self,
        name: &'static str,
        arity: usize,
        f: impl Fn(&mut EngineState, &mut Vm, &[Value]) -> Result<Value, Value> + 'static,
    ) {
        let es = Rc::clone(self.es);
        self.set(
            name,
            sync_native(name, arity, move |vm, _this, args| {
                f(&mut es.borrow_mut(), vm, args)
            }),
        );
    }

    fn async_op(
        &self,
        name: &'static str,
        arity: usize,
        f: impl Fn(&Rc<RefCell<EngineState>>, &Rc<RefCell<PendingOps>>, &mut Vm, &[Value], HostPromise)
            + 'static,
    ) {
        let es = Rc::clone(self.es);
        let pending = Rc::clone(self.pending);
        self.set(
            name,
            async_native(name, arity, move |vm, _this, args, handle| {
                f(&es, &pending, vm, args, handle);
            }),
        );
    }
}

fn size_array((w, h): (u32, u32)) -> Value {
    Value::new_array(vec![Value::Number(w as f64), Value::Number(h as f64)])
}

/// ops をインストールし、`globalThis.Deno.core.ops` へ生やす。
pub(super) fn install_ops(
    interp: &mut Interpreter,
    es: &Rc<RefCell<EngineState>>,
    pending: &Rc<RefCell<PendingOps>>,
) {
    let ops_obj = Value::new_plain_object();
    let reg = OpRegistrar {
        ops_obj: &ops_obj,
        es,
        pending,
    };
    install_layer_ops(&reg);
    install_text_ops(&reg);
    install_trans_ops(&reg);
    install_audio_ops(&reg);
    install_video_ops(&reg);
    install_input_ops(&reg);
    install_sys_ops(&reg);

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

fn install_layer_ops(reg: &OpRegistrar<'_>) {
    reg.es("op_layer_new", 1, |es, vm, args| {
        let kind = as_string(&arg(args, 0));
        es.layer_new(&kind)
            .map(|id| Value::Number(id as f64))
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_free", 1, |es, _vm, args| {
        es.layer_free(as_u32(&arg(args, 0)));
        Ok(Value::Undefined)
    });
    reg.es("op_layer_load_image", 3, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        let path = as_string(&arg(args, 2));
        es.layer_load_image(id, &page, path)
            .map(size_array)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_load_image_ex", 4, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        let path = as_string(&arg(args, 2));
        let opts = image_load_opts_from_value(vm, &arg(args, 3))?;
        es.layer_load_image_ex(id, &page, path, opts)
            .map(size_array)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_clear_image", 2, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        es.layer_clear_image(id, &page)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_fill", 8, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        let w = as_u32(&arg(args, 2));
        let h = as_u32(&arg(args, 3));
        let r = as_u8(&arg(args, 4));
        let g = as_u8(&arg(args, 5));
        let b = as_u8(&arg(args, 6));
        let a = as_u8(&arg(args, 7));
        es.layer_fill(id, &page, w, h, r, g, b, a)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_msgbox", 9, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        let w = as_u32(&arg(args, 2));
        let h = as_u32(&arg(args, 3));
        let radius = as_u32(&arg(args, 4));
        let r = as_u8(&arg(args, 5));
        let g = as_u8(&arg(args, 6));
        let b = as_u8(&arg(args, 7));
        let a = as_u8(&arg(args, 8));
        es.layer_msgbox(id, &page, w, h, radius, r, g, b, a)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_panel", 3, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        let spec: Vec<f64> = tsint::vm::array_values(&arg(args, 2))
            .iter()
            .map(num)
            .collect();
        es.layer_panel(id, &page, &spec)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_load_image_scaled", 5, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        let path = as_string(&arg(args, 2));
        let w = as_u32(&arg(args, 3));
        let h = as_u32(&arg(args, 4));
        es.layer_load_image_scaled(id, &page, path, w, h)
            .map(size_array)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_set_props", 3, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        let props = layer_props_from_value(vm, &arg(args, 2))?;
        es.layer_set_props(id, &page, props)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_move", 3, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        let move_args = move_args_from_value(vm, &arg(args, 2))?;
        es.layer_move(id, &page, move_args)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_set_crop", 3, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        let w = as_u32(&arg(args, 2));
        es.layer_set_crop(id, &page, w)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_crop_move", 5, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        let to = as_u32(&arg(args, 2));
        let time_ms = to_time_ms(num(&arg(args, 3)));
        let accel = num(&arg(args, 4)) as f32;
        es.layer_crop_move(id, &page, to, time_ms, accel)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_move_cancel", 2, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        es.layer_move_cancel(id, &page)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_move_path", 4, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        let points = move_args_array_from_value(vm, &arg(args, 2))?;
        let spline = as_bool(&arg(args, 3));
        es.layer_move_path(id, &page, points, spline)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_move_path_by_distance", 5, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        let points = move_args_array_from_value(vm, &arg(args, 2))?;
        let spline = as_bool(&arg(args, 3));
        let total_time_ms = num(&arg(args, 4));
        es.layer_move_path_by_distance(id, &page, points, spline, total_time_ms)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_move_skip", 0, |es, _vm, _args| {
        es.layer_move_skip();
        Ok(Value::Undefined)
    });
    reg.es("op_layer_moving", 0, |es, _vm, _args| {
        Ok(Value::Bool(es.layer_moving()))
    });
    reg.es("op_backlay", 1, |es, _vm, args| {
        es.backlay(as_i32(&arg(args, 0)));
        Ok(Value::Undefined)
    });
    reg.es("op_layer_copy", 5, |es, vm, args| {
        let src_id = as_u32(&arg(args, 0));
        let src_page = as_string(&arg(args, 1));
        let dst_id = as_u32(&arg(args, 2));
        let dst_page = as_string(&arg(args, 3));
        let copy_position = as_bool(&arg(args, 4));
        es.layer_copy(src_id, &src_page, dst_id, &dst_page, copy_position)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_layer_blit_image", 6, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        let path = as_string(&arg(args, 2));
        let dx = as_i32(&arg(args, 3));
        let dy = as_i32(&arg(args, 4));
        let clip = blit_clip_from_value(vm, &arg(args, 5))?;
        es.layer_blit_image(id, &page, &path, dx, dy, clip)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_quake", 3, |es, _vm, args| {
        es.quake(
            as_i32(&arg(args, 0)),
            as_i32(&arg(args, 1)),
            as_u32(&arg(args, 2)),
        );
        Ok(Value::Undefined)
    });
    reg.es("op_quake_stop", 0, |es, _vm, _args| {
        es.quake_stop();
        Ok(Value::Undefined)
    });
}

fn install_text_ops(reg: &OpRegistrar<'_>) {
    reg.es_mut("op_text_style", 2, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let style = text_style_from_value(vm, &arg(args, 1))?;
        es.text_style(id, style);
        Ok(Value::Undefined)
    });
    reg.es_mut("op_text_print", 4, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let segments = segments_from_value(vm, &arg(args, 1))?;
        let append = as_bool(&arg(args, 2));
        let cps = num(&arg(args, 3));
        es.text_print(id, segments, append, cps)
            .map(print_result_to_value)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es_mut("op_text_reflow", 1, |es, vm, args| {
        es.text_reflow(as_u32(&arg(args, 0)))
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_text_complete", 1, |es, _vm, args| {
        es.text_complete(as_u32(&arg(args, 0)));
        Ok(Value::Undefined)
    });
    reg.es("op_text_is_done", 1, |es, _vm, args| {
        Ok(Value::Bool(es.text_is_done(as_u32(&arg(args, 0)))))
    });
    reg.es_mut("op_text_clear", 1, |es, _vm, args| {
        es.text_clear(as_u32(&arg(args, 0)));
        Ok(Value::Undefined)
    });
    reg.es("op_text_rects", 1, |es, _vm, args| {
        Ok(glyph_rects_to_value(es.text_rects(as_u32(&arg(args, 0)))))
    });
}

fn install_trans_ops(reg: &OpRegistrar<'_>) {
    reg.es("op_trans_begin", 1, |es, vm, args| {
        let a = trans_args_from_value(vm, &arg(args, 0))?;
        es.trans_begin(a)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_trans_skip", 0, |es, _vm, _args| {
        es.trans_skip();
        Ok(Value::Undefined)
    });
    reg.async_op("op_trans_wait", 0, |es, pending, vm, _args, handle| {
        if es.borrow().trans_active() {
            pending.borrow_mut().trans.push(handle);
        } else {
            handle.resolve(vm, Value::Undefined);
        }
    });
}

fn install_audio_ops(reg: &OpRegistrar<'_>) {
    reg.es("op_audio_play", 7, |es, vm, args| {
        let ch = as_string(&arg(args, 0));
        let path = as_string(&arg(args, 1));
        let looping = as_bool(&arg(args, 2));
        let volume = num(&arg(args, 3));
        let fade_in_ms = as_u32(&arg(args, 4));
        let start_position_ms = num(&arg(args, 5));
        let start_paused = as_bool(&arg(args, 6));
        es.audio_play(
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
    });
    reg.es("op_audio_stop", 2, |es, vm, args| {
        let ch = as_string(&arg(args, 0));
        es.audio_stop(&ch, as_u32(&arg(args, 1)))
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_audio_position", 1, |es, vm, args| {
        let ch = as_string(&arg(args, 0));
        es.audio_position(&ch)
            .map(|position| Value::Number(position as f64))
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_audio_volume", 2, |es, vm, args| {
        let ch = as_string(&arg(args, 0));
        es.audio_volume(&ch, num(&arg(args, 1)))
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_audio_is_playing", 1, |es, vm, args| {
        let ch = as_string(&arg(args, 0));
        es.audio_is_playing(&ch)
            .map(Value::Bool)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_audio_pause", 1, |es, vm, args| {
        let ch = as_string(&arg(args, 0));
        es.audio_pause(&ch)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_audio_resume", 1, |es, vm, args| {
        let ch = as_string(&arg(args, 0));
        es.audio_resume(&ch)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_audio_fade_to", 4, |es, vm, args| {
        let ch = as_string(&arg(args, 0));
        let volume = num(&arg(args, 1));
        let ms = as_u32(&arg(args, 2));
        let then = as_string(&arg(args, 3));
        es.audio_fade_to(&ch, volume, ms, &then)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_audio_is_fading", 1, |es, vm, args| {
        let ch = as_string(&arg(args, 0));
        es.audio_is_fading(&ch)
            .map(Value::Bool)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_audio_is_finished", 1, |es, vm, args| {
        let ch = as_string(&arg(args, 0));
        es.audio_is_finished(&ch)
            .map(Value::Bool)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.async_op("op_audio_wait_end", 1, |_es, pending, vm, args, handle| {
        let ch_name = as_string(&arg(args, 0));
        match channel_index(&ch_name) {
            Some(ch) => pending.borrow_mut().audio.push((ch, handle)),
            None => {
                let e = vm.make_error("Error", format!("不明なチャンネル: {ch_name}"));
                handle.reject(vm, e);
            }
        }
    });
}

fn install_video_ops(reg: &OpRegistrar<'_>) {
    reg.es_mut("op_video_play", 3, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let path = as_string(&arg(args, 1));
        let looping = as_bool(&arg(args, 2));
        es.video_play(id, &path, looping)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es_mut("op_video_play_at", 5, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let path = as_string(&arg(args, 1));
        let looping = as_bool(&arg(args, 2));
        let frame = video_optional_frame(num(&arg(args, 3)))
            .map_err(|e| vm.make_error("Error", e))?;
        let position_ms = num(&arg(args, 4));
        es.video_play_at(
            id,
            &path,
            looping,
            frame,
            (position_ms != -1.0).then_some(position_ms),
        )
        .map(|_| Value::Undefined)
        .map_err(|e| vm.make_error("Error", e))
    });
    reg.es_mut("op_video_stop", 0, |es, _vm, _args| {
        es.video_stop();
        Ok(Value::Undefined)
    });
    reg.es("op_video_is_playing", 0, |es, _vm, _args| {
        Ok(Value::Bool(es.video_is_playing()))
    });
    reg.es_mut("op_video_open", 3, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let path = as_string(&arg(args, 1));
        let looping = as_bool(&arg(args, 2));
        es.video_open(id, &path, looping)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es_mut("op_video_prepare", 3, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let path = as_string(&arg(args, 1));
        let looping = as_bool(&arg(args, 2));
        es.video_prepare(id, &path, looping)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es_mut("op_video_prepare_opened", 1, |es, vm, args| {
        es.video_prepare_opened(as_u32(&arg(args, 0)))
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es_mut("op_video_play_prepared", 1, |es, vm, args| {
        es.video_play_prepared(as_u32(&arg(args, 0)))
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es_mut("op_video_pause", 0, |es, _vm, _args| {
        es.video_pause();
        Ok(Value::Undefined)
    });
    reg.es_mut("op_video_resume", 0, |es, _vm, _args| {
        es.video_resume();
        Ok(Value::Undefined)
    });
    reg.es_mut("op_video_rewind", 0, |es, _vm, _args| {
        es.video_rewind();
        Ok(Value::Undefined)
    });
    reg.es("op_video_is_paused", 0, |es, _vm, _args| {
        Ok(Value::Bool(es.video_is_paused()))
    });
    reg.es_mut("op_video_seek_frame", 1, |es, vm, args| {
        es.video_seek_frame(as_u32(&arg(args, 0)) as usize)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es_mut("op_video_seek_position", 1, |es, vm, args| {
        es.video_seek_position(num(&arg(args, 0)))
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es_mut("op_video_set_period_event", 1, |es, _vm, args| {
        es.video_set_period_event(as_u32(&arg(args, 0)) as usize);
        Ok(Value::Undefined)
    });
    reg.es_mut("op_video_cancel_period_event", 0, |es, _vm, _args| {
        es.video_cancel_period_event();
        Ok(Value::Undefined)
    });
    reg.es_mut("op_video_set_segment_loop", 2, |es, vm, args| {
        let start = as_u32(&arg(args, 0)) as usize;
        let end = as_u32(&arg(args, 1)) as usize;
        es.video_set_segment_loop(start, end)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es_mut("op_video_cancel_segment_loop", 0, |es, _vm, _args| {
        es.video_cancel_segment_loop();
        Ok(Value::Undefined)
    });
    reg.es_mut("op_video_take_event", 1, |es, _vm, args| {
        let kind = as_string(&arg(args, 0));
        Ok(Value::Bool(es.video_take_event(&kind)))
    });
    reg.es("op_video_generation", 0, |es, _vm, _args| {
        Ok(Value::Number(es.video_generation()))
    });
}

fn install_input_ops(reg: &OpRegistrar<'_>) {
    reg.async_op("op_input_next_event", 0, |_es, pending, vm, _args, handle| {
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
    });
}

fn install_sys_ops(reg: &OpRegistrar<'_>) {
    reg.es("op_log", 2, |es, _vm, args| {
        let level = as_string(&arg(args, 0));
        let msg = as_string(&arg(args, 1));
        es.sys_log(&level, &msg);
        Ok(Value::Undefined)
    });
    reg.es("op_now", 0, |es, _vm, _args| Ok(Value::Number(es.now())));
    reg.async_op("op_wait", 1, |es, pending, _vm, args, handle| {
        let ms = num(&arg(args, 0));
        // 期限は EngineState::clock 基準: windowed（Clock::Real）は実時間、
        // ヘッドレス（Clock::Virtual）は仮想時刻に対して決定的になる。
        let deadline = es.borrow().clock.now() + wait_duration(ms);
        pending.borrow_mut().timers.push((deadline, handle));
    });
    reg.es("op_quit", 0, |es, _vm, _args| {
        es.quit();
        Ok(Value::Undefined)
    });
    reg.es("op_set_title", 1, |es, _vm, args| {
        es.set_title(as_string(&arg(args, 0)));
        Ok(Value::Undefined)
    });
    reg.es("op_read_text", 1, |es, vm, args| {
        let rel = as_string(&arg(args, 0));
        es.read_text(&rel)
            .map(Value::str)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_file_exists", 1, |es, _vm, args| {
        Ok(Value::Bool(es.file_exists(&as_string(&arg(args, 0)))))
    });
    reg.es("op_save_write", 2, |es, vm, args| {
        let slot = as_string(&arg(args, 0));
        let json = as_string(&arg(args, 1));
        es.save_write(&slot, &json)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_save_read", 1, |es, vm, args| {
        let slot = as_string(&arg(args, 0));
        es.save_read(&slot)
            .map(|opt| match opt {
                Some(s) => Value::str(s),
                None => Value::Null,
            })
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_save_list", 0, |es, vm, _args| {
        es.save_list()
            .map(slot_list_to_value)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_save_delete", 1, |es, vm, args| {
        let slot = as_string(&arg(args, 0));
        es.save_delete(&slot)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_screenshot", 1, |es, vm, args| {
        let slot = as_string(&arg(args, 0));
        es.screenshot(&slot)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_thumb_capture", 2, |es, vm, args| {
        es.thumb_capture_sized(as_u32(&arg(args, 0)), as_u32(&arg(args, 1)))
            .map(|()| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_prefetch_save_thumbs", 2, |es, _vm, args| {
        es.prefetch_save_thumbs(as_u32(&arg(args, 0)), as_u32(&arg(args, 1)));
        Ok(Value::Undefined)
    });
    reg.es("op_prefetch_images_scaled", 3, |es, _vm, args| {
        let list = as_string(&arg(args, 0))
            .split('\u{a}')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        es.prefetch_images_scaled(list, as_u32(&arg(args, 1)), as_u32(&arg(args, 2)));
        Ok(Value::Undefined)
    });
    reg.es("op_thumb_save", 1, |es, vm, args| {
        let slot = as_string(&arg(args, 0));
        es.thumb_save(&slot)
            .map(|_| Value::Undefined)
            .map_err(|e| vm.make_error("Error", e))
    });
    reg.es("op_wall_clock_string", 0, |es, _vm, _args| {
        Ok(Value::str(es.wall_clock_string()))
    });
    reg.es("op_layer_load_save_thumb", 5, |es, vm, args| {
        let id = as_u32(&arg(args, 0));
        let page = as_string(&arg(args, 1));
        let slot = as_string(&arg(args, 2));
        let w = as_u32(&arg(args, 3));
        let h = as_u32(&arg(args, 4));
        es.layer_load_save_thumb_sized(id, &page, &slot, w, h)
            .map(size_array)
            .map_err(|e| vm.make_error("Error", e))
    });
}
