//! `globalThis.Deno.core.ops` へ登録する tsint ネイティブ ops 群。

use std::cell::RefCell;
use std::rc::Rc;

use noiria_core::audio::channel_index;
use tsint::value::Value;
use tsint::vm::{HostPromise, Vm};
use tsint::Interpreter;

use crate::script::engine_state::{video_optional_frame, EngineError, EngineState};
use crate::script::numconv::{to_time_ms, wait_duration};

use super::convert::*;
use super::event_loop::PendingOps;

type EsFn = fn(&EngineState, &mut Vm, Args<'_>) -> Result<Value, Value>;
type EsMutFn = fn(&mut EngineState, &mut Vm, Args<'_>) -> Result<Value, Value>;
type AsyncOpFn =
    fn(&Rc<RefCell<EngineState>>, &Rc<RefCell<PendingOps>>, &mut Vm, Args<'_>, HostPromise);

#[derive(Clone, Copy)]
struct Args<'a>(&'a [Value]);

impl<'a> Args<'a> {
    fn val(self, i: usize) -> &'a Value {
        arg(self.0, i)
    }
    fn u32(self, i: usize) -> u32 {
        as_u32(self.val(i))
    }
    fn i32(self, i: usize) -> i32 {
        as_i32(self.val(i))
    }
    fn coord(self, i: usize) -> i32 {
        as_coord(self.val(i))
    }
    fn u8(self, i: usize) -> u8 {
        as_u8(self.val(i))
    }
    fn f64(self, i: usize) -> f64 {
        num(self.val(i))
    }
    fn bool(self, i: usize) -> bool {
        as_bool(self.val(i))
    }
    fn str(self, i: usize) -> String {
        as_string(self.val(i))
    }
}

// クロージャの単相化を op 単位で展開しないため、関数ポインタ 1 型に束ねて登録する。
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

    fn es_table(&self, entries: &[(&'static str, usize, EsFn)]) {
        for &(name, arity, f) in entries {
            let es = Rc::clone(self.es);
            self.set(
                name,
                Value::native_fn(
                    name,
                    arity,
                    Rc::new(move |vm, _this, args| f(&es.borrow(), vm, Args(args))),
                ),
            );
        }
    }

    fn es_mut_table(&self, entries: &[(&'static str, usize, EsMutFn)]) {
        for &(name, arity, f) in entries {
            let es = Rc::clone(self.es);
            self.set(
                name,
                Value::native_fn(
                    name,
                    arity,
                    Rc::new(move |vm, _this, args| f(&mut es.borrow_mut(), vm, Args(args))),
                ),
            );
        }
    }

    fn async_table(&self, entries: &[(&'static str, usize, AsyncOpFn)]) {
        for &(name, arity, f) in entries {
            let es = Rc::clone(self.es);
            let pending = Rc::clone(self.pending);
            self.set(
                name,
                Value::native_fn(
                    name,
                    arity,
                    Rc::new(move |vm, _this, args| {
                        let (promise_val, handle) = vm.new_host_promise();
                        f(&es, &pending, vm, Args(args), handle);
                        Ok(promise_val)
                    }),
                ),
            );
        }
    }
}

fn size_array((w, h): (u32, u32)) -> Value {
    Value::new_array(vec![Value::Number(w as f64), Value::Number(h as f64)])
}

fn unit(vm: &mut Vm, r: Result<(), EngineError>) -> Result<Value, Value> {
    r.map(|()| Value::Undefined)
        .map_err(|e| vm.make_error("Error", e))
}

fn map_val<T>(vm: &mut Vm, r: Result<T, EngineError>, f: fn(T) -> Value) -> Result<Value, Value> {
    r.map(f).map_err(|e| vm.make_error("Error", e))
}

fn ok_undef() -> Result<Value, Value> {
    Ok(Value::Undefined)
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
    reg.es_table(ES_OPS);
    reg.es_mut_table(ES_MUT_OPS);
    reg.async_table(ASYNC_OPS);

    let deno_obj = obj_from_pairs([("core", obj_from_pairs([("ops", ops_obj)]))]);
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

const ES_OPS: &[(&str, usize, EsFn)] = &[
    // layer
    ("op_layer_new", 1, |es, vm, a| {
        map_val(vm, es.layer_new(&a.str(0)), |id| Value::Number(id as f64))
    }),
    ("op_layer_free", 1, |es, _, a| {
        es.layer_free(a.u32(0));
        ok_undef()
    }),
    ("op_layer_load_image", 3, |es, vm, a| {
        map_val(
            vm,
            es.layer_load_image(a.u32(0), &a.str(1), a.str(2)),
            size_array,
        )
    }),
    ("op_layer_load_image_ex", 4, |es, vm, a| {
        let opts = image_load_opts_from_value(vm, a.val(3))?;
        map_val(
            vm,
            es.layer_load_image_ex(a.u32(0), &a.str(1), a.str(2), opts),
            size_array,
        )
    }),
    ("op_layer_clear_image", 2, |es, vm, a| {
        unit(vm, es.layer_clear_image(a.u32(0), &a.str(1)))
    }),
    ("op_layer_fill", 8, |es, vm, a| {
        unit(
            vm,
            es.layer_fill(
                a.u32(0),
                &a.str(1),
                a.u32(2),
                a.u32(3),
                a.u8(4),
                a.u8(5),
                a.u8(6),
                a.u8(7),
            ),
        )
    }),
    ("op_layer_msgbox", 9, |es, vm, a| {
        unit(
            vm,
            es.layer_msgbox(
                a.u32(0),
                &a.str(1),
                a.u32(2),
                a.u32(3),
                a.u32(4),
                a.u8(5),
                a.u8(6),
                a.u8(7),
                a.u8(8),
            ),
        )
    }),
    ("op_layer_panel", 3, |es, vm, a| {
        let spec: Vec<f64> = tsint::vm::array_values(a.val(2)).iter().map(num).collect();
        unit(vm, es.layer_panel(a.u32(0), &a.str(1), &spec))
    }),
    ("op_layer_load_image_scaled", 5, |es, vm, a| {
        let r = es.layer_load_image_scaled(a.u32(0), &a.str(1), a.str(2), a.u32(3), a.u32(4));
        map_val(vm, r, size_array)
    }),
    ("op_layer_set_props", 3, |es, vm, a| {
        let props = layer_props_from_value(vm, a.val(2))?;
        unit(vm, es.layer_set_props(a.u32(0), &a.str(1), props))
    }),
    ("op_layer_move", 3, |es, vm, a| {
        let mv = move_args_from_value(vm, a.val(2))?;
        unit(vm, es.layer_move(a.u32(0), &a.str(1), mv))
    }),
    ("op_layer_set_crop", 3, |es, vm, a| {
        unit(vm, es.layer_set_crop(a.u32(0), &a.str(1), a.u32(2)))
    }),
    ("op_layer_crop_move", 5, |es, vm, a| {
        let r = es.layer_crop_move(
            a.u32(0),
            &a.str(1),
            a.u32(2),
            to_time_ms(a.f64(3)),
            a.f64(4) as f32,
        );
        unit(vm, r)
    }),
    ("op_layer_move_cancel", 2, |es, vm, a| {
        unit(vm, es.layer_move_cancel(a.u32(0), &a.str(1)))
    }),
    ("op_layer_move_path", 4, |es, vm, a| {
        let pts = move_args_array_from_value(vm, a.val(2))?;
        unit(vm, es.layer_move_path(a.u32(0), &a.str(1), pts, a.bool(3)))
    }),
    ("op_layer_move_path_by_distance", 5, |es, vm, a| {
        let pts = move_args_array_from_value(vm, a.val(2))?;
        let r = es.layer_move_path_by_distance(a.u32(0), &a.str(1), pts, a.bool(3), a.f64(4));
        unit(vm, r)
    }),
    ("op_layer_move_skip", 0, |es, _, _| {
        es.layer_move_skip();
        ok_undef()
    }),
    ("op_layer_moving", 0, |es, _, _| {
        Ok(Value::Bool(es.layer_moving()))
    }),
    ("op_backlay", 1, |es, _, a| {
        es.backlay(a.i32(0));
        ok_undef()
    }),
    ("op_layer_copy", 5, |es, vm, a| {
        let r = es.layer_copy(a.u32(0), &a.str(1), a.u32(2), &a.str(3), a.bool(4));
        unit(vm, r)
    }),
    ("op_layer_blit_image", 6, |es, vm, a| {
        let clip = blit_clip_from_value(vm, a.val(5))?;
        let r = es.layer_blit_image(a.u32(0), &a.str(1), &a.str(2), a.coord(3), a.coord(4), clip);
        unit(vm, r)
    }),
    ("op_quake", 3, |es, _, a| {
        es.quake(a.coord(0), a.coord(1), a.u32(2));
        ok_undef()
    }),
    ("op_quake_stop", 0, |es, _, _| {
        es.quake_stop();
        ok_undef()
    }),
    // text
    ("op_text_complete", 1, |es, _, a| {
        es.text_complete(a.u32(0));
        ok_undef()
    }),
    ("op_text_is_done", 1, |es, _, a| {
        Ok(Value::Bool(es.text_is_done(a.u32(0))))
    }),
    ("op_text_rects", 1, |es, _, a| {
        Ok(glyph_rects_to_value(es.text_rects(a.u32(0))))
    }),
    // transition
    ("op_trans_begin", 1, |es, vm, a| {
        let args = trans_args_from_value(vm, a.val(0))?;
        unit(vm, es.trans_begin(args))
    }),
    ("op_trans_skip", 0, |es, _, _| {
        es.trans_skip();
        ok_undef()
    }),
    // audio
    ("op_audio_play", 7, |es, vm, a| {
        let r = es.audio_play(
            &a.str(0),
            &a.str(1),
            a.bool(2),
            a.f64(3),
            a.u32(4),
            a.f64(5),
            a.bool(6),
        );
        unit(vm, r)
    }),
    ("op_audio_stop", 2, |es, vm, a| {
        unit(vm, es.audio_stop(&a.str(0), a.u32(1)))
    }),
    ("op_audio_position", 1, |es, vm, a| {
        map_val(vm, es.audio_position(&a.str(0)), |p| {
            Value::Number(p as f64)
        })
    }),
    ("op_audio_volume", 2, |es, vm, a| {
        unit(vm, es.audio_volume(&a.str(0), a.f64(1)))
    }),
    ("op_audio_is_playing", 1, |es, vm, a| {
        map_val(vm, es.audio_is_playing(&a.str(0)), Value::Bool)
    }),
    ("op_audio_pause", 1, |es, vm, a| {
        unit(vm, es.audio_pause(&a.str(0)))
    }),
    ("op_audio_resume", 1, |es, vm, a| {
        unit(vm, es.audio_resume(&a.str(0)))
    }),
    ("op_audio_fade_to", 4, |es, vm, a| {
        unit(
            vm,
            es.audio_fade_to(&a.str(0), a.f64(1), a.u32(2), &a.str(3)),
        )
    }),
    ("op_audio_is_fading", 1, |es, vm, a| {
        map_val(vm, es.audio_is_fading(&a.str(0)), Value::Bool)
    }),
    ("op_audio_is_finished", 1, |es, vm, a| {
        map_val(vm, es.audio_is_finished(&a.str(0)), Value::Bool)
    }),
    // video (read-only)
    ("op_video_is_playing", 0, |es, _, _| {
        Ok(Value::Bool(es.video_is_playing()))
    }),
    ("op_video_is_paused", 0, |es, _, _| {
        Ok(Value::Bool(es.video_is_paused()))
    }),
    ("op_video_generation", 0, |es, _, _| {
        Ok(Value::Number(es.video_generation()))
    }),
    // sys
    ("op_log", 2, |es, _, a| {
        es.sys_log(&a.str(0), &a.str(1));
        ok_undef()
    }),
    ("op_now", 0, |es, _, _| Ok(Value::Number(es.now()))),
    ("op_quit", 0, |es, _, _| {
        es.quit();
        ok_undef()
    }),
    ("op_set_title", 1, |es, _, a| {
        es.set_title(a.str(0));
        ok_undef()
    }),
    ("op_read_text", 1, |es, vm, a| {
        map_val(vm, es.read_text(&a.str(0)), Value::str)
    }),
    ("op_file_exists", 1, |es, _, a| {
        Ok(Value::Bool(es.file_exists(&a.str(0))))
    }),
    ("op_save_write", 2, |es, vm, a| {
        unit(vm, es.save_write(&a.str(0), &a.str(1)))
    }),
    ("op_save_read", 1, |es, vm, a| {
        map_val(vm, es.save_read(&a.str(0)), |opt| {
            opt.map_or(Value::Null, Value::str)
        })
    }),
    ("op_save_list", 0, |es, vm, _| {
        map_val(vm, es.save_list(), slot_list_to_value)
    }),
    ("op_save_delete", 1, |es, vm, a| {
        unit(vm, es.save_delete(&a.str(0)))
    }),
    ("op_screenshot", 1, |es, vm, a| {
        unit(vm, es.screenshot(&a.str(0)))
    }),
    ("op_thumb_capture", 2, |es, vm, a| {
        unit(vm, es.thumb_capture_sized(a.u32(0), a.u32(1)))
    }),
    ("op_prefetch_save_thumbs", 2, |es, _, a| {
        es.prefetch_save_thumbs(a.u32(0), a.u32(1));
        ok_undef()
    }),
    ("op_prefetch_images_scaled", 3, |es, _, a| {
        let list = a
            .str(0)
            .split('\n')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        es.prefetch_images_scaled(list, a.u32(1), a.u32(2));
        ok_undef()
    }),
    ("op_thumb_save", 1, |es, vm, a| {
        unit(vm, es.thumb_save(&a.str(0)))
    }),
    ("op_wall_clock_string", 0, |es, _, _| {
        Ok(Value::str(es.wall_clock_string()))
    }),
    ("op_layer_load_save_thumb", 5, |es, vm, a| {
        let r = es.layer_load_save_thumb_sized(a.u32(0), &a.str(1), &a.str(2), a.u32(3), a.u32(4));
        map_val(vm, r, size_array)
    }),
];

const ES_MUT_OPS: &[(&str, usize, EsMutFn)] = &[
    // text
    ("op_text_style", 2, |es, vm, a| {
        let style = text_style_from_value(vm, a.val(1))?;
        es.text_style(a.u32(0), style);
        ok_undef()
    }),
    ("op_text_print", 4, |es, vm, a| {
        let segs = segments_from_value(vm, a.val(1))?;
        map_val(
            vm,
            es.text_print(a.u32(0), segs, a.bool(2), a.f64(3)),
            print_result_to_value,
        )
    }),
    ("op_text_reflow", 1, |es, vm, a| {
        unit(vm, es.text_reflow(a.u32(0)))
    }),
    ("op_text_clear", 1, |es, _, a| {
        es.text_clear(a.u32(0));
        ok_undef()
    }),
    // video
    ("op_video_play", 3, |es, vm, a| {
        unit(vm, es.video_play(a.u32(0), &a.str(1), a.bool(2)))
    }),
    ("op_video_play_at", 5, |es, vm, a| {
        let frame = video_optional_frame(a.f64(3)).map_err(|e| vm.make_error("Error", e))?;
        let pos = a.f64(4);
        let r = es.video_play_at(
            a.u32(0),
            &a.str(1),
            a.bool(2),
            frame,
            (pos != -1.0).then_some(pos),
        );
        unit(vm, r)
    }),
    ("op_video_stop", 0, |es, _, _| {
        es.video_stop();
        ok_undef()
    }),
    ("op_video_open", 3, |es, vm, a| {
        unit(vm, es.video_open(a.u32(0), &a.str(1), a.bool(2)))
    }),
    ("op_video_prepare", 3, |es, vm, a| {
        unit(vm, es.video_prepare(a.u32(0), &a.str(1), a.bool(2)))
    }),
    ("op_video_prepare_opened", 1, |es, vm, a| {
        unit(vm, es.video_prepare_opened(a.u32(0)))
    }),
    ("op_video_play_prepared", 1, |es, vm, a| {
        unit(vm, es.video_play_prepared(a.u32(0)))
    }),
    ("op_video_pause", 0, |es, _, _| {
        es.video_pause();
        ok_undef()
    }),
    ("op_video_resume", 0, |es, _, _| {
        es.video_resume();
        ok_undef()
    }),
    ("op_video_rewind", 0, |es, _, _| {
        es.video_rewind();
        ok_undef()
    }),
    ("op_video_seek_frame", 1, |es, vm, a| {
        unit(vm, es.video_seek_frame(a.u32(0) as usize))
    }),
    ("op_video_seek_position", 1, |es, vm, a| {
        unit(vm, es.video_seek_position(a.f64(0)))
    }),
    ("op_video_set_period_event", 1, |es, _, a| {
        es.video_set_period_event(a.u32(0) as usize);
        ok_undef()
    }),
    ("op_video_cancel_period_event", 0, |es, _, _| {
        es.video_cancel_period_event();
        ok_undef()
    }),
    ("op_video_set_segment_loop", 2, |es, vm, a| {
        unit(
            vm,
            es.video_set_segment_loop(a.u32(0) as usize, a.u32(1) as usize),
        )
    }),
    ("op_video_cancel_segment_loop", 0, |es, _, _| {
        es.video_cancel_segment_loop();
        ok_undef()
    }),
    ("op_video_take_event", 1, |es, _, a| {
        Ok(Value::Bool(es.video_take_event(&a.str(0))))
    }),
];

const ASYNC_OPS: &[(&str, usize, AsyncOpFn)] = &[
    ("op_trans_wait", 0, |es, pending, vm, _, handle| {
        if es.borrow().trans_active() {
            pending.borrow_mut().trans.push(handle);
        } else {
            handle.resolve(vm, Value::Undefined);
        }
    }),
    ("op_audio_wait_end", 1, |_, pending, vm, a, handle| {
        let ch_name = a.str(0);
        match channel_index(&ch_name) {
            Some(ch) => pending.borrow_mut().audio.push((ch, handle)),
            None => {
                let e = vm.make_error("Error", format!("不明なチャンネル: {ch_name}"));
                handle.reject(vm, e);
            }
        }
    }),
    ("op_input_next_event", 0, |_, pending, vm, _, handle| {
        let mut p = pending.borrow_mut();
        if let Some(ev) = p.input_buffer.pop_front() {
            drop(p);
            handle.resolve(vm, input_event_to_value(&ev));
        } else if p.input_closed {
            drop(p);
            handle.resolve(vm, quit_event_value());
        } else {
            p.input = Some(handle);
            p.input_acked = false;
        }
    }),
    ("op_wait", 1, |es, pending, _, a, handle| {
        let deadline = es.borrow().clock.now() + wait_duration(a.f64(0));
        pending.borrow_mut().timers.push((deadline, handle));
    }),
];
