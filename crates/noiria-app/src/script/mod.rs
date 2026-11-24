//! スクリプトスレッド: tokio current-thread + LocalSet 上で V8 (JsRuntime) を動かす。

#[cfg(all(test, feature = "engine-v8", feature = "engine-tsint"))]
mod b1_tests;
#[cfg(all(test, feature = "engine-v8", feature = "engine-tsint"))]
mod b2_tests;
#[cfg(all(test, feature = "engine-v8", feature = "engine-tsint"))]
mod b3_tests;
pub(crate) mod engine_state;
#[cfg(feature = "engine-v8")]
mod loader;
mod numconv;
#[cfg(feature = "engine-v8")]
mod ops_audio;
#[cfg(feature = "engine-v8")]
mod ops_input;
#[cfg(feature = "engine-v8")]
mod ops_layer;
#[cfg(feature = "engine-v8")]
mod ops_sys;
#[cfg(feature = "engine-v8")]
mod ops_text;
#[cfg(feature = "engine-v8")]
mod ops_trans;
#[cfg(feature = "engine-v8")]
mod ops_video;
#[cfg(all(test, feature = "engine-v8", feature = "engine-tsint"))]
mod r2_tests;
mod runtime_modules;
#[cfg(feature = "engine-v8")]
mod state;
#[cfg(all(test, feature = "engine-v8", feature = "engine-tsint"))]
mod tag_behavior_tests;
#[cfg(all(test, feature = "engine-v8", feature = "engine-tsint"))]
mod tag_harness;
#[cfg(feature = "engine-tsint")]
pub(crate) mod tsint_engine;
mod ui_cache;
#[cfg(feature = "engine-v8")]
mod util;

use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::Sender;
use noiria_core::assets::{GameConfig, GameDir};
use noiria_core::audio::{AudioCmd, AudioShared};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::Notify;

use crate::bridge::{InputEvent, InputSync, PendingWaits, SharedScene, TickSync, WakeSink};
use crate::clock::Clock;

#[cfg(feature = "engine-v8")]
use std::rc::Rc;
#[cfg(feature = "engine-v8")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "engine-v8")]
use std::task::{Wake, Waker};
#[cfg(feature = "engine-v8")]
use std::thread::JoinHandle;

#[cfg(feature = "engine-v8")]
use anyhow::Context;
#[cfg(feature = "engine-v8")]
use deno_core::{extension, JsRuntime, PollEventLoopOptions, RuntimeOptions};

#[cfg(feature = "engine-v8")]
use crate::bridge::UserEvent;
#[cfg(feature = "engine-v8")]
use engine_state::EngineState;
#[cfg(feature = "engine-v8")]
use loader::NoiriaModuleLoader;
#[cfg(feature = "engine-v8")]
use state::InputRx;

#[cfg(feature = "engine-v8")]
struct DrainWake {
    parent: Waker,
    woke: AtomicBool,
}

#[cfg(feature = "engine-v8")]
impl Wake for DrainWake {
    fn wake(self: Arc<Self>) {
        self.woke.store(true, Ordering::SeqCst);
        self.parent.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woke.store(true, Ordering::SeqCst);
        self.parent.wake_by_ref();
    }
}

/// 拡張へ渡す初期化コンテキスト。
pub(crate) struct EngineCtx {
    pub(crate) scene: SharedScene,
    pub(crate) proxy: WakeSink,
    pub(crate) input_rx: UnboundedReceiver<InputEvent>,
    pub(crate) trans_notify: Arc<Notify>,
    pub(crate) audio_tx: Sender<AudioCmd>,
    pub(crate) audio_shared: Arc<AudioShared>,
    pub(crate) game_dir: GameDir,
    pub(crate) config: GameConfig,
    pub(crate) start: Instant,
    /// 現在時刻の供給源（`crate::clock::Clock`）。windowed 実行は `Clock::Real`
    /// （挙動不変）、ヘッドレスは `crate::main` が作る `Clock::Virtual` を共有する
    /// （`docs/implementation-notes.md`「2026-07-13 dist フレーム差異の調査と
    /// headless 決定論化」参照）。
    pub(crate) clock: Clock,
    /// ヘッドレス専用のロックステップ同期カウンタ（`crate::bridge::InputSync` 参照）。
    /// windowed 実行は生成するだけで誰も観測しない（挙動不変）。
    pub(crate) input_sync: InputSync,
    /// tick ドレインバリア用カウンタ（`crate::bridge::TickSync` 参照）。tsint の
    /// `drive()` ループに加え、V8 の `ops_sys.rs::op_wait`（`EngineState.tick_sync`
    /// 経由）も watch バリアの完了報告（`TickSync::react`）に使う。windowed は
    /// 生成するだけで誰も観測しない（挙動不変）。
    pub(crate) tick_sync: TickSync,
    /// ヘッドレス（V8 専用）の `op_wait` 仮想 in-flight カウンタ
    /// （`crate::bridge::PendingWaits` 参照）。tsint/windowed は生成するだけで誰も
    /// 観測しない（挙動不変）。
    pub(crate) pending_waits: PendingWaits,
    /// タグ振る舞いテストハーネス専用の ops 呼び出し記録先（テスト専用）。
    #[cfg(test)]
    pub(crate) op_log: Option<engine_state::SharedOpLog>,
}

#[cfg(feature = "engine-v8")]
extension!(
    noiria_ext,
    ops = [
        // layer
        ops_layer::op_layer_new,
        ops_layer::op_layer_free,
        ops_layer::op_layer_load_image,
        ops_layer::op_layer_load_image_ex,
        ops_layer::op_layer_clear_image,
        ops_layer::op_layer_fill,
        ops_layer::op_layer_msgbox,
        ops_layer::op_layer_panel,
        ops_layer::op_layer_load_image_scaled,
        ops_layer::op_layer_set_props,
        ops_layer::op_layer_move,
        ops_layer::op_layer_move_cancel,
        ops_layer::op_layer_crop_move,
        ops_layer::op_layer_set_crop,
        ops_layer::op_layer_move_path,
        ops_layer::op_layer_move_path_by_distance,
        ops_layer::op_layer_move_skip,
        ops_layer::op_layer_moving,
        ops_layer::op_backlay,
        ops_layer::op_layer_copy,
        ops_layer::op_layer_blit_image,
        ops_layer::op_quake,
        ops_layer::op_quake_stop,
        // text
        ops_text::op_text_style,
        ops_text::op_text_print,
        ops_text::op_text_reflow,
        ops_text::op_text_complete,
        ops_text::op_text_is_done,
        ops_text::op_text_clear,
        ops_text::op_text_rects,
        // transition
        ops_trans::op_trans_begin,
        ops_trans::op_trans_skip,
        ops_trans::op_trans_wait,
        // audio
        ops_audio::op_audio_play,
        ops_audio::op_audio_stop,
        ops_audio::op_audio_volume,
        ops_audio::op_audio_is_playing,
        ops_audio::op_audio_position,
        ops_audio::op_audio_pause,
        ops_audio::op_audio_resume,
        ops_audio::op_audio_fade_to,
        ops_audio::op_audio_is_fading,
        ops_audio::op_audio_is_finished,
        ops_audio::op_audio_wait_end,
        // video
        ops_video::op_video_play,
        ops_video::op_video_play_at,
        ops_video::op_video_stop,
        ops_video::op_video_is_playing,
        ops_video::op_video_open,
        ops_video::op_video_prepare,
        ops_video::op_video_prepare_opened,
        ops_video::op_video_play_prepared,
        ops_video::op_video_pause,
        ops_video::op_video_resume,
        ops_video::op_video_rewind,
        ops_video::op_video_is_paused,
        ops_video::op_video_seek_frame,
        ops_video::op_video_seek_position,
        ops_video::op_video_set_period_event,
        ops_video::op_video_cancel_period_event,
        ops_video::op_video_set_segment_loop,
        ops_video::op_video_cancel_segment_loop,
        ops_video::op_video_take_event,
        ops_video::op_video_generation,
        // input
        ops_input::op_input_next_event,
        // sys
        ops_sys::op_log,
        ops_sys::op_now,
        ops_sys::op_wait,
        ops_sys::op_quit,
        ops_sys::op_set_title,
        ops_sys::op_read_text,
        ops_sys::op_file_exists,
        ops_sys::op_save_write,
        ops_sys::op_save_read,
        ops_sys::op_save_list,
        ops_sys::op_save_delete,
        ops_sys::op_screenshot,
        ops_sys::op_thumb_capture,
        ops_sys::op_prefetch_save_thumbs,
        ops_sys::op_prefetch_images_scaled,
        ops_sys::op_thumb_save,
        ops_sys::op_wall_clock_string,
        ops_layer::op_layer_load_save_thumb,
    ],
    options = { ctx: EngineCtx },
    state = |state, opt| {
        let (engine_state, input_rx) = EngineState::from_ctx(opt.ctx);
        state.put(InputRx(Rc::new(tokio::sync::Mutex::new(input_rx))));
        state.put(state::InputWaitEpoch::default());
        state.put(engine_state);
    },
);

/// スクリプトスレッドを起動する。スクリプト終了（正常/異常とも）でメインループへ Quit を送る。
#[cfg(feature = "engine-v8")]
pub(crate) fn spawn(ctx: EngineCtx) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("noiria-script".into())
        .spawn(move || {
            let proxy = ctx.proxy.clone();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("tokio ランタイムの作成に失敗");
            let local = tokio::task::LocalSet::new();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                local.block_on(&rt, run_script(ctx))
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => crate::fatal::record_script_error(format!("{e:#}")),
                Err(p) => crate::fatal::record_script_error(crate::fatal::panic_message(&*p)),
            }
            proxy.send(UserEvent::Quit);
        })
        .expect("スクリプトスレッドの起動に失敗")
}

#[cfg(feature = "engine-v8")]
async fn run_script(ctx: EngineCtx) -> anyhow::Result<()> {
    let main_rel = ctx.config.main.clone();

    let loader = Rc::new(NoiriaModuleLoader::new(ctx.game_dir.clone())?);
    let main_url = loader
        .main_module(&main_rel)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut js = JsRuntime::new(RuntimeOptions {
        extensions: vec![noiria_ext::init(ctx)],
        module_loader: Some(loader),
        ..Default::default()
    });

    // CJK フォントの起動時チェック（失敗しても続行、ログのみ）
    {
        let op_state = js.op_state();
        let mut op_state = op_state.borrow_mut();
        let engine_state = op_state.borrow_mut::<EngineState>();
        let style = noiria_core::text::TextStyle::default();
        if let Err(e) = engine_state.texts.engine.assert_cjk(&style) {
            log::warn!("{e:#}");
        }
    }

    // ヘッドレスのロックステップ同期用: 入力待ちの世代カウンタ（`state.rs` の
    // `InputWaitEpoch`）と ack 先（`EngineState::input_sync`）を取り出しておく。
    let (wait_epoch, input_sync, tick_sync) = {
        let op_state = js.op_state();
        let op_state = op_state.borrow();
        (
            op_state.borrow::<state::InputWaitEpoch>().clone(),
            op_state.borrow::<EngineState>().input_sync.clone(),
            op_state.borrow::<EngineState>().tick_sync.clone(),
        )
    };

    let mod_id = js
        .load_main_es_module(&main_url)
        .await
        .with_context(|| format!("メインモジュールの読み込みに失敗: {main_url}"))?;
    log::debug!("メインモジュール読み込み完了: {main_url}");
    let eval = js.mod_evaluate(mod_id);
    // `js.run_event_loop(..).await` 相当の手動ポーリングループ。ヘッドレス時は入力待ちの
    // ack と、期限へ到達した V8 `op_wait` の Promise 継続を drain した後の tick 完了を
    // この外側で報告する。windowed 実行でも同じコードパスを通るが、ack/完了世代は
    // 誰も観測しないため挙動は不変。
    //
    // 期限前の `op_wait` は外側のイベントループを起こさないため、自身で tick 反応を
    // 報告する。期限到達後は deno_core が即時 work のため current waker を再予約する間、
    // 保留反応を flush しない。両方を外側だけで扱うと複数 tick の wait がハングし、
    // 両方を op 内だけで扱うと継続が次 tick へずれる。
    let mut acked_epoch = 0u64;
    std::future::poll_fn(|cx| {
        let drain_wake = Arc::new(DrainWake {
            parent: cx.waker().clone(),
            woke: AtomicBool::new(false),
        });
        let tracked_waker = Waker::from(drain_wake.clone());
        let mut tracked_cx = std::task::Context::from_waker(&tracked_waker);
        let poll = js.poll_event_loop(&mut tracked_cx, PollEventLoopOptions::default());
        let scheduled_more = drain_wake.woke.load(Ordering::SeqCst);
        if tick_sync.has_deferred_reactors() && (poll.is_ready() || !scheduled_more) {
            tick_sync.flush_deferred();
        }
        if poll.is_pending() && !scheduled_more {
            let epoch = wait_epoch.0.get();
            if epoch > acked_epoch {
                acked_epoch = epoch;
                input_sync.ack();
            }
        }
        poll
    })
    .await
    .context("スクリプトのイベントループでエラー")?;
    log::info!("スクリプトのイベントループが終了しました");
    eval.await.context("メインモジュールの評価でエラー")?;
    Ok(())
}

#[cfg(all(test, feature = "engine-v8"))]
mod js_tests {
    use super::*;
    use deno_core::ModuleSpecifier;

    /// V8 + ModuleLoader だけでエンジン組み込み TS を実行するヘルパ。
    /// アサーションは JS 側で throw し、失敗はモジュール評価エラーとして検出する。
    /// W5: `NoiriaModuleLoader` が `GameDir` を要求するようになったので、ここでも
    /// `game.json` だけの最小ゲームディレクトリを一時的に用意する（このテストは
    /// 埋め込み `noiria:` モジュールしか読まないので中身は使われない）。
    fn run_js_module(code: &str) -> anyhow::Result<()> {
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("noiria-js-module-test-{ns}"));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("game.json"),
            r#"{"title":"t","id":"noiria-js-module-test"}"#,
        )?;
        let (game_dir, _config) = noiria_core::assets::GameDir::open(&dir)?;

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?;
        let result = tokio::task::LocalSet::new().block_on(&rt, async {
            let loader = Rc::new(NoiriaModuleLoader::new(game_dir)?);
            let mut js = JsRuntime::new(RuntimeOptions {
                module_loader: Some(loader),
                ..Default::default()
            });
            let spec = ModuleSpecifier::parse("file:///noiria-test/main.ts")?;
            let id = js
                .load_main_es_module_from_code(&spec, code.to_string())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let eval = js.mod_evaluate(id);
            js.run_event_loop(PollEventLoopOptions::default())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            eval.await.map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(())
        });
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    const ASSERT_JS: &str = r#"
function assertEq(actual, expected, msg) {
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${msg}: ${a} !== ${b}`);
}
function assert(cond, msg) { if (!cond) throw new Error(msg); }
"#;

    #[test]
    fn kag_parser_parses_fixture_in_v8() {
        let fixture = "; コメント行は無視\n\
*start|はじまり\n\
@playbgm storage=main.wav loop=true\n\
こんにちは[l][r]\n\
[image layer=base storage=\"部屋 の絵.png\" left=10]\n\
[emb exp=\"f.x + 1\"]\n\
[[エスケープ]も本文\n\
[link target=*end]選択肢[endlink]\n\
*end\n";
        let code = format!(
            r#"
import {{ parseScenario }} from "noiria:kag/parser";
{ASSERT_JS}
const src = {src};
const sc = parseScenario(src, "test.ks");

assertEq([...sc.labels.keys()], ["start", "end"], "ラベル一覧");
assertEq(sc.labels.get("start"), 0, "start の位置");

const kinds = sc.commands.map(c =>
  c.type === "tag" ? "tag:" + c.name : c.type === "label" ? "label:" + c.name : "text:" + c.text);
assertEq(kinds, [
  "label:start",
  "tag:playbgm",
  "text:こんにちは", "tag:l", "tag:r",
  "tag:image",
  "tag:emb",
  "text:[エスケープ]も本文",
  "tag:link", "text:選択肢", "tag:endlink",
  "label:end",
], "コマンド列");

const playbgm = sc.commands[1];
assertEq(playbgm.attrs, {{ storage: "main.wav", loop: "true" }}, "playbgm 属性");

const image = sc.commands[5];
assertEq(image.attrs.storage, "部屋 の絵.png", "クォート内の空白を保持");
assertEq(image.attrs.left, "10", "裸の属性値");

const emb = sc.commands[6];
assertEq(emb.attrs.exp, "f.x + 1", "式属性");

const label = sc.commands[0];
assertEq(label.display, "はじまり", "ラベル表示名");
"#,
            src = serde_json::to_string(fixture).unwrap(),
        );
        run_js_module(&code).unwrap();
    }

    #[test]
    fn kag_vars_eval_in_v8() {
        let code = format!(
            r#"
import {{ f, evalExpr, evalStmt, replaceF }} from "noiria:kag/vars";
{ASSERT_JS}
evalStmt("f.x = 2 + 3", null);
assertEq(evalExpr("f.x * 2", null), 10, "eval 結果");
assertEq(f.x, 5, "f へ書き込まれる");

evalStmt("f.name = 'アリス'", null);
assertEq(evalExpr("f.name + 'さん'", null), "アリスさん", "文字列式");

replaceF({{ y: 1 }});
assertEq(f.x, undefined, "replaceF が古いキーを消す");
assertEq(f.y, 1, "replaceF が新しい値を入れる");

// sys オブジェクトへのアクセス
assertEq(evalExpr("sys.answer", {{ answer: 42 }}), 42, "sys 参照");

// kag グローバル（W7、KAG3 標準。sys.player と同一インスタンスを想定した第 3 引数）。
assertEq(evalExpr("kag.answer", null, {{ answer: 7 }}), 7, "kag 参照");
const kagObj = {{ hit: false }};
evalStmt("kag.hit = true", null, kagObj);
assertEq(kagObj.hit, true, "evalStmt も kag 経由で副作用を起こせる");
"#
        );
        run_js_module(&code).unwrap();
    }

    #[test]
    fn kag_player_settings_pure_functions_in_v8() {
        let code = format!(
            r#"
import {{ effectiveVolume, clampVolPct, stepStage, CPS_STAGES, AUTO_STAGES, nextMenuMode }} from "noiria:kag/player";
{ASSERT_JS}

// 実効音量 = タグの volume (0..1) x 設定の割合 (0..100)
assertEq(effectiveVolume(0.7, 100), 0.7, "100% はタグの volume をそのまま通す");
assertEq(effectiveVolume(1, 50), 0.5, "50% で半分になる");
assertEq(effectiveVolume(1, 0), 0, "0% で無音");
assertEq(effectiveVolume(0, 80), 0, "タグの volume が 0 なら常に無音");

assertEq(clampVolPct(120), 100, "上限クランプ");
assertEq(clampVolPct(-5), 0, "下限クランプ");
assertEq(clampVolPct(80), 80, "範囲内はそのまま");

// cps (文字速度) の 4 段階: 遅い20 → 普通35 → 速い60 → 瞬間0
assertEq(CPS_STAGES, [20, 35, 60, 0], "cps 段階の並び");
assertEq(stepStage(CPS_STAGES, 35, 1), 60, "普通 → 速い");
assertEq(stepStage(CPS_STAGES, 35, -1), 20, "普通 → 遅い");
assertEq(stepStage(CPS_STAGES, 60, 1), 0, "速い → 瞬間");
assertEq(stepStage(CPS_STAGES, 0, 1), 0, "瞬間より先へは進まない（端で停止）");
assertEq(stepStage(CPS_STAGES, 20, -1), 20, "遅いより前へは戻らない（端で停止）");

// オート待ち時間の 3 段階: 短800 → 普通1800 → 長3200
assertEq(AUTO_STAGES, [800, 1800, 3200], "autoWaitMs 段階の並び");
assertEq(stepStage(AUTO_STAGES, 1800, 1), 3200, "普通 → 長");
assertEq(stepStage(AUTO_STAGES, 1800, -1), 800, "普通 → 短");

// メニュー見出しのモード循環: セーブ → ロード → 設定 → セーブ
assertEq(nextMenuMode("save"), "load", "セーブ→ロード");
assertEq(nextMenuMode("load"), "settings", "ロード→設定");
assertEq(nextMenuMode("settings"), "save", "設定→セーブ");
"#
        );
        run_js_module(&code).unwrap();
    }

    #[test]
    fn kag_player_name_plate_pure_functions_in_v8() {
        let code = format!(
            r#"
import {{ namePlateWidth, computeNamePlateCfg }} from "noiria:kag/player";
{ASSERT_JS}

// 幅は文字数×フォントサイズ＋左右マージンだが [MIN, MAX] にクランプされる
assertEq(namePlateWidth("ア", 20, 14), 88, "1 文字でも最小幅を下回らない");
assertEq(namePlateWidth("アリス", 20, 14), 88, "短い名前は最小幅にクランプ");
assertEq(namePlateWidth("とても長い名前のキャラクターです", 20, 14), 220, "長い名前は最大幅にクランプ");

const msg = {{
  left: 40, top: 500, width: 1200, height: 200, radius: 14,
  color: [10, 12, 30, 195], fontSize: 26, marginX: 30, marginY: 18, rows: 4,
}};
const cfg = computeNamePlateCfg(msg, "アリス");
assert(cfg.fontSize < msg.fontSize, "名前欄フォントは本文よりひと回り小さい");
assertEq(cfg.left, msg.left + 16, "left はウィンドウ left + 16");
assert(cfg.top < msg.top, "top はウィンドウ top より上");
assert(cfg.top + cfg.height > msg.top, "下端はウィンドウ上端と少し重なる");
assertEq(cfg.height, Math.ceil(cfg.fontSize * 1.6) + 6 * 2, "高さ=フォントサイズ×行送り係数(切上)+上下マージン×2");

// 名前が変わらない限り再計算しても位置は安定する
const cfg2 = computeNamePlateCfg(msg, "アリス");
assertEq(cfg, cfg2, "同じ入力に対して決定的");
"#
        );
        run_js_module(&code).unwrap();
    }

    /// W3: `migrateSaveV1toV2` 純関数の単体テスト（V8 js_tests パターン）。
    #[test]
    fn kag_player_save_v1_to_v2_migration_in_v8() {
        let code = format!(
            r#"
import {{ migrateSaveV1toV2 }} from "noiria:kag/player";
{ASSERT_JS}

const v1 = {{
  version: 1,
  pos: {{ file: "a.ks", label: "start" }},
  f: {{ x: 1 }},
  layers: [],
  bgm: null,
  name: "アリス",
  date: "d",
  title: "t",
}};
const v2 = migrateSaveV1toV2(v1);
assertEq(v2.version, 2, "v1 は v2 へ引き上げられる");
assertEq(v2.msgLayers, [], "v1 には msgLayers が無いので空配列で補完");
assertEq(v2.current, 0, "v1 には current が無いので 0 で補完");
assertEq(v2.name, "アリス", "他フィールドは保持される");

// 既に v2 以上のデータはそのまま返す（msgLayers/current を上書きしない）
const already = {{
  ...v1,
  version: 2,
  msgLayers: [{{ cfg: {{}}, visible: false, nameLabel: "ボブ" }}],
  current: 1,
}};
const same = migrateSaveV1toV2(already);
assertEq(same.version, 2, "v2 はそのまま");
assertEq(same.current, 1, "v2 の current は上書きしない");
assertEq(same.msgLayers.length, 1, "v2 の msgLayers は上書きしない");
"#
        );
        run_js_module(&code).unwrap();
    }

    #[test]
    fn kag_parser_valueless_and_at_tags() {
        let fixture = "@cm\n@wait time=300\n[nowait][endnowait]\n";
        let code = format!(
            r#"
import {{ parseScenario }} from "noiria:kag/parser";
{ASSERT_JS}
const sc = parseScenario({src}, "t.ks");
const kinds = sc.commands.map(c => c.type + ":" + (c.name ?? ""));
assertEq(kinds, ["tag:cm", "tag:wait", "tag:nowait", "tag:endnowait"], "行タグとインラインタグ");
assertEq(sc.commands[1].attrs.time, "300", "@タグの属性");
"#,
            src = serde_json::to_string(fixture).unwrap(),
        );
        run_js_module(&code).unwrap();
    }

    /// W6: [iscript]/[endscript] が生テキストを 1 コマンドとして収集し、`[`/`]` を含む
    /// TS コードもタグとして誤解釈しないことを確認する（parser.ts の新モード）。
    #[test]
    fn kag_parser_iscript_collects_raw_body_in_v8() {
        let fixture = "*start\n\
[eval exp=\"f.before = 1\"]\n\
[iscript]\n\
const arr = [1, 2, 3];\n\
f.total = arr[0] + arr[1];\n\
sys.log(\"from iscript\");\n\
[endscript]\n\
[eval exp=\"f.after = 1\"]\n\
おわり[l]\n";
        let code = format!(
            r#"
import {{ parseScenario }} from "noiria:kag/parser";
{ASSERT_JS}
const sc = parseScenario({src}, "t.ks");
const kinds = sc.commands.map(c => c.type + (c.type === "tag" ? ":" + c.name : ""));
assertEq(kinds, [
  "label", "tag:eval", "iscript", "tag:eval", "text", "tag:l",
], "iscript は 1 コマンドへ折り畳まれ、前後のタグは通常通り解釈される");

const iscript = sc.commands[2];
assert(iscript.code.includes("const arr = [1, 2, 3];"), "本文の [ ] がそのまま保持される（タグとして解釈されない）");
assert(iscript.code.includes("arr[0] + arr[1]"), "配列インデックスの [ ] も保持される");
assert(!iscript.code.includes("[endscript]"), "終端行そのものは本文に含まれない");
"#,
            src = serde_json::to_string(fixture).unwrap(),
        );
        run_js_module(&code).unwrap();
    }

    /// W6: `scanIgnoreBlock`（player.ts）の純関数テスト。if/ignore を同じ depth に含める
    /// 入れ子走査が、ネストした [if]/[ignore] の組み合わせでも正しく対応するエンドを返すこと
    /// を確認する（`kag_anim_and_move_path_pure_functions_in_v8` と同じ「ops 不要」パターン）。
    #[test]
    fn kag_player_scan_ignore_block_pure_function_in_v8() {
        let code = format!(
            r#"
import {{ scanIgnoreBlock }} from "noiria:kag/player";
{ASSERT_JS}

function tag(name) {{ return {{ type: "tag", name, attrs: {{}}, line: 0 }}; }}
function text(t) {{ return {{ type: "text", text: t, line: 0 }}; }}

// 単純な [ignore]...[endignore]
{{
  const cmds = [tag("ignore"), text("skip me"), tag("endignore"), tag("l")];
  assertEq(scanIgnoreBlock(cmds, 1), 3, "endignore の次の index を返す");
}}

// 入れ子の [if]...[endif] を跨いで対応する [endignore] まで飛ぶ
{{
  const cmds = [
    tag("ignore"),      // 0
    tag("if"),           // 1
    text("nested"),      // 2
    tag("endif"),         // 3
    tag("endignore"),     // 4
    tag("l"),             // 5
  ];
  assertEq(scanIgnoreBlock(cmds, 1), 5, "入れ子の if/endif を depth に含めて正しく対応する endignore を見つける");
}}

// 入れ子の [ignore]...[endignore] 自体も正しく数える
{{
  const cmds = [
    tag("ignore"),        // 0 (outer, 呼び出し側はここから1を渡す想定)
    tag("ignore"),        // 1 (inner)
    text("inner"),        // 2
    tag("endignore"),     // 3 (inner を閉じる)
    text("outer"),        // 4
    tag("endignore"),     // 5 (outer を閉じる)
    tag("l"),              // 6
  ];
  assertEq(scanIgnoreBlock(cmds, 1), 6, "内側の endignore で depth を戻し、外側の endignore まで正しく飛ぶ");
}}

// 対応する endignore が無ければ commands.length を返す
{{
  const cmds = [tag("ignore"), text("unterminated")];
  assertEq(scanIgnoreBlock(cmds, 1), cmds.length, "閉じタグが無ければ末尾まで");
}}
"#
        );
        run_js_module(&code).unwrap();
    }

    /// W4: [button]/[checkbox]/[edit] の純関数（`stateClip`/`hitRect`/`applyEditKey`）の
    /// 単体テスト（ops 不要、`kag_player_settings_pure_functions_in_v8` と同じパターン）。
    #[test]
    fn kag_buttons_pure_functions_in_v8() {
        let code = format!(
            r#"
import {{ stateClip, hitRect, applyEditKey }} from "noiria:kag/buttons";
{ASSERT_JS}

// 3 状態画像 (1 状態ぶんの幅 stateW=40, 高さ h=60) の中から該当スライスを切り出す clip。
assertEq(stateClip(40, 60, "normal"), {{ x: 0, y: 0, w: 40, h: 60 }}, "normal は先頭スライス");
assertEq(stateClip(40, 60, "over"), {{ x: 40, y: 0, w: 40, h: 60 }}, "over は 2 番目のスライス");
assertEq(stateClip(40, 60, "pressed"), {{ x: 80, y: 0, w: 40, h: 60 }}, "pressed は 3 番目のスライス");

assert(hitRect(5, 5, 0, 0, 10, 10), "矩形内はヒット");
assert(!hitRect(10, 5, 0, 0, 10, 10), "右端ちょうどはヒットしない（半開区間）");
assert(hitRect(0, 0, 0, 0, 10, 10), "左上端はヒット");
assert(!hitRect(-1, 5, 0, 0, 10, 10), "矩形の外（左）はヒットしない");

assertEq(applyEditKey("ab", "c", 10), "abc", "1 文字キーは追記される");
assertEq(applyEditKey("abc", "Backspace", 10), "ab", "Backspace は末尾を削る");
assertEq(applyEditKey("", "Backspace", 10), "", "空バッファの Backspace は無害");
assertEq(applyEditKey("abcdefghij", "k", 10), "abcdefghij", "maxLen 到達で追記されない");
assertEq(applyEditKey("ab", "Enter", 10), "ab", "名前付きキー(複数文字)は無視される");
assertEq(applyEditKey("ab", "あ", 10), "abあ", "非 ASCII の 1 コードポイントも追記できる");
"#
        );
        run_js_module(&code).unwrap();
    }

    /// W5: `parseAsd`（anim.ts）/`parseMovePath`（tags.ts）純関数の単体テスト
    /// （`kag_buttons_pure_functions_in_v8` と同じパターン、ops 不要）。
    #[test]
    fn kag_anim_and_move_path_pure_functions_in_v8() {
        let code = format!(
            r#"
import {{ parseAsd }} from "noiria:kag/anim";
import {{ parseMovePath, resolvePimageClip }} from "noiria:kag/tags";
{ASSERT_JS}

// .asd 最小サブセット: loop=/コメント/空行/セル定義 (セル番号,graphic,wait)。
const asd = parseAsd(`
; コメント行
loop=false

0,icon_a.png,350
1,icon_b.png,200
`);
assertEq(asd.loop, false, "loop=false を読み取る");
assertEq(asd.cells, [
  {{ path: "icon_a.png", wait: 350 }},
  {{ path: "icon_b.png", wait: 200 }},
], "セルは記述順で読み取られる");

// loop= 省略時は既定 true。
assertEq(parseAsd("0,a.png,100").loop, true, "loop 省略時は既定 true");
// セル未満の行 (カンマ不足) は無視される。
assertEq(parseAsd("broken,line\n0,a.png,100").cells.length, 1, "不正な行は無視される");

// move path=: (x,y,opacity) 複数点、opacity は省略可。
assertEq(
  parseMovePath("(100,50,255)(300,50)"),
  [{{ x: 100, y: 50, opacity: 255 }}, {{ x: 300, y: 50 }}],
  "opacity 省略時は undefined のまま",
);
assertEq(parseMovePath("no parens here"), [], "括弧が無ければ空配列");
assertEq(parseMovePath("(1,2)(bad)(3,4)"), [{{ x: 1, y: 2 }}, {{ x: 3, y: 4 }}], "不正な点は無視される");
assertEq(
  parseMovePath("(0,0)(100,-100)"),
  [{{ x: 0, y: 0 }}, {{ x: 100, y: -100 }}],
  "spline=true でも指定点は順序と符号を保ってパースされる",
);

// pimage の sx=/sy=/sw=/sh=（W7、KAG3 標準の clip= 別名）。
assertEq(
  resolvePimageClip({{ sx: "1", sy: "2", sw: "3", sh: "4" }}),
  {{ x: 1, y: 2, w: 3, h: 4 }},
  "sx/sy/sw/sh から clip 矩形を組み立てる",
);
assertEq(resolvePimageClip({{ clip: "5,6,7,8" }}), {{ x: 5, y: 6, w: 7, h: 8 }}, "clip= が優先される");
assertEq(resolvePimageClip({{}}), null, "どちらも無指定なら null（全体を使う）");
"#
        );
        run_js_module(&code).unwrap();
    }

    /// W4: `ButtonManager` の実 ops 版 end-to-end テスト（`move_tween_reaches_to_value_in_v8`
    /// と同じ「実ファイル + 実 GameDir + 実 EngineState」方式）。3 状態ボタン画像
    /// （赤/緑/青の 4x4px を横に並べた 12x4 のスプライト）を使い、ヒットテスト・
    /// active/locked ゲーティング・状態遷移 (normal/over/pressed) を検証する。
    #[test]
    fn button_manager_hit_test_and_state_transitions_in_v8() -> anyhow::Result<()> {
        use std::sync::Mutex;

        use crossbeam_channel::unbounded;
        use noiria_core::assets::save_png;
        use noiria_core::pixmap::Pixmap;
        use noiria_core::scene::Scene;

        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("noiria-v8-buttons-test-{ns}"));
        std::fs::create_dir_all(dir.join("assets/fg"))?;
        std::fs::write(
            dir.join("game.json"),
            r#"{"title":"t","id":"noiria-v8-buttons-test","main":"main.ts"}"#,
        )?;
        let mut sprite = Pixmap::new(12, 4);
        sprite.fill([0, 0, 0, 0]);
        let red = Pixmap::from_rgba_straight(4, 4, &[255, 0, 0, 255].repeat(4 * 4));
        let green = Pixmap::from_rgba_straight(4, 4, &[0, 255, 0, 255].repeat(4 * 4));
        let blue = Pixmap::from_rgba_straight(4, 4, &[0, 0, 255, 255].repeat(4 * 4));
        sprite.blit_over(&red, 0, 0, 255);
        sprite.blit_over(&green, 4, 0, 255);
        sprite.blit_over(&blue, 8, 0, 255);
        save_png(&sprite, &dir.join("assets/fg/button.png"))?;

        let (game_dir, config) = noiria_core::assets::GameDir::open(dir.clone())?;
        let scene: SharedScene = Arc::new(Mutex::new(Scene::new(64, 64)));
        let (_input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
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

        let code = r#"
import { ButtonManager } from "noiria:kag/buttons";
const ops = globalThis.Deno.core.ops;
function assertEq(actual, expected, msg) {
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${msg}: ${a} !== ${b}`);
}
function assert(cond, msg) { if (!cond) throw new Error(msg); }

const mgr = new ButtonManager();
const w = mgr.addButton(0, 0, "assets/fg/button.png", "*label", null, "f.x = 1");
assertEq(w.state, "normal", "初期状態は normal");
assertEq([w.x, w.y, w.w, w.h], [0, 0, 4, 4], "1 状態ぶんの寸法 = 画像幅/3");

// [s] 停止中でない (active=false) 間はホバー/クリックとも無反応。
assertEq(mgr.click(2, 2), null, "非活性時はヒットしない");
assertEq(w.state, "normal", "非活性時は状態も変わらない");

mgr.active = true;
mgr.hover(100, 100); // 範囲外
assertEq(w.state, "normal", "範囲外のホバーでは normal のまま");
mgr.hover(2, 2); // 範囲内 (0,0,4,4)
assertEq(w.state, "over", "範囲内のホバーで over に遷移");

const action = mgr.click(2, 2);
assertEq(action, { kind: "button", target: "*label", storage: null, exp: "f.x = 1" }, "クリックのアクション");
assertEq(w.state, "pressed", "クリックで pressed に遷移");

// [locklink] 相当のロック中はヒットしない。
w.state = "normal";
mgr.setGlobalLocked(true);
assertEq(mgr.click(2, 2), null, "globalLocked 中はヒットしない");
assertEq(w.state, "normal", "ロック中は状態も変わらない");
mgr.setGlobalLocked(false);

ops.op_quit();
"#;

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?;
        let run = tokio::task::LocalSet::new().block_on(&rt, async {
            let loader = Rc::new(NoiriaModuleLoader::new(ctx.game_dir.clone())?);
            let mut js = JsRuntime::new(RuntimeOptions {
                extensions: vec![noiria_ext::init(ctx)],
                module_loader: Some(loader),
                ..Default::default()
            });
            let spec = ModuleSpecifier::parse("file:///noiria-test/main.ts")?;
            let id = js
                .load_main_es_module_from_code(&spec, code.to_string())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let eval = js.mod_evaluate(id);
            js.run_event_loop(PollEventLoopOptions::default())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            eval.await.map_err(|e| anyhow::anyhow!("{e}"))?;
            anyhow::Ok(())
        });
        std::fs::remove_dir_all(&dir).ok();
        run?;
        Ok(())
    }

    /// V8 実 ops 版の end-to-end テスト（tsint 側 `tsint_ops_wiring_runs_a_small_script_to_completion`
    /// 相当）: [move] 開始 → 十分に待つ → `op_layer_moving()` が false → レンダラの
    /// 1 周期分に相当する `settle_finished_tweens` 後にレイヤの位置/不透明度が
    /// to 値になっていることを確認する。
    #[test]
    fn move_tween_reaches_to_value_in_v8() -> anyhow::Result<()> {
        use std::sync::Mutex;

        use crossbeam_channel::unbounded;
        use noiria_core::scene::{Page, Scene};

        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("noiria-v8-move-test-{ns}"));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("game.json"),
            r#"{"title":"t","id":"noiria-v8-move-test","main":"main.ts"}"#,
        )?;
        let (game_dir, config) = noiria_core::assets::GameDir::open(dir.clone())?;

        let scene: SharedScene = Arc::new(Mutex::new(Scene::new(64, 64)));
        let (_input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
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

        let code = r#"
const ops = globalThis.Deno.core.ops;
const id = ops.op_layer_new("image");
ops.op_layer_set_props(id, "fore", { x: 0, y: 0, opacity: 0, visible: true });
ops.op_layer_move(id, "fore", { toX: 100, toOpacity: 255, timeMs: 40 });
await ops.op_wait(80);
if (ops.op_layer_moving()) throw new Error("moving は tween の duration 経過後は false のはず");
ops.op_quit();
"#;

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?;
        let run = tokio::task::LocalSet::new().block_on(&rt, async {
            let loader = Rc::new(NoiriaModuleLoader::new(ctx.game_dir.clone())?);
            let mut js = JsRuntime::new(RuntimeOptions {
                extensions: vec![noiria_ext::init(ctx)],
                module_loader: Some(loader),
                ..Default::default()
            });
            let spec = ModuleSpecifier::parse("file:///noiria-test/main.ts")?;
            let id = js
                .load_main_es_module_from_code(&spec, code.to_string())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let eval = js.mod_evaluate(id);
            js.run_event_loop(PollEventLoopOptions::default())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            eval.await.map_err(|e| anyhow::anyhow!("{e}"))?;
            anyhow::Ok(())
        });
        std::fs::remove_dir_all(&dir).ok();
        run?;

        // 完了した tween の確定は renderer の責務: 1 周期ぶんを模して呼ぶ。
        let mut scene = scene.lock().unwrap();
        let now = Instant::now();
        scene.settle_finished_tweens(now);
        let layer = scene.layer(Page::Fore, 1).expect("レイヤが作られていない");
        assert!(layer.tween.is_none(), "settle 後は tween が外れているはず");
        assert_eq!(
            (layer.x, layer.opacity),
            (100, 255),
            "位置/不透明度が to 値になっているはず"
        );
        Ok(())
    }

    /// [move path=]（W5）の V8 実 ops 版配線テスト（tsint 側
    /// `tsint_move_path_wiring_starts_first_leg_and_queues_rest` 相当）: `op_layer_move_path`
    /// が第一区間を tween、残りを tween_queue へ積むことを確認する。
    #[test]
    fn move_path_tween_wiring_starts_first_leg_and_queues_rest_in_v8() -> anyhow::Result<()> {
        use std::sync::Mutex;

        use crossbeam_channel::unbounded;
        use noiria_core::scene::{Page, Scene};

        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("noiria-v8-move-path-test-{ns}"));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("game.json"),
            r#"{"title":"t","id":"noiria-v8-move-path-test","main":"main.ts"}"#,
        )?;
        let (game_dir, config) = noiria_core::assets::GameDir::open(dir.clone())?;

        let scene: SharedScene = Arc::new(Mutex::new(Scene::new(64, 64)));
        let (_input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
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

        let code = r#"
const ops = globalThis.Deno.core.ops;
const id = ops.op_layer_new("image");
ops.op_layer_set_props(id, "fore", { x: 0, y: 0, opacity: 0, visible: true });
ops.op_layer_move_path(id, "fore", [
  { toX: 50, timeMs: 40 },
  { toX: 150, toY: 80, toOpacity: 128, timeMs: 40 },
], false);
ops.op_quit();
"#;

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?;
        let run = tokio::task::LocalSet::new().block_on(&rt, async {
            let loader = Rc::new(NoiriaModuleLoader::new(ctx.game_dir.clone())?);
            let mut js = JsRuntime::new(RuntimeOptions {
                extensions: vec![noiria_ext::init(ctx)],
                module_loader: Some(loader),
                ..Default::default()
            });
            let spec = ModuleSpecifier::parse("file:///noiria-test/main.ts")?;
            let id = js
                .load_main_es_module_from_code(&spec, code.to_string())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let eval = js.mod_evaluate(id);
            js.run_event_loop(PollEventLoopOptions::default())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            eval.await.map_err(|e| anyhow::anyhow!("{e}"))?;
            anyhow::Ok(())
        });
        std::fs::remove_dir_all(&dir).ok();
        run?;

        let scene = scene.lock().unwrap();
        let layer = scene.layer(Page::Fore, 1).expect("レイヤが作られていない");
        let t = layer
            .tween
            .as_ref()
            .expect("第一区間の tween が開始しているはず");
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
        Ok(())
    }

    #[test]
    fn virtual_wait_continuation_drains_before_tick_completion_in_v8() -> anyhow::Result<()> {
        use std::sync::Mutex;
        use std::time::Duration;

        use crossbeam_channel::unbounded;
        use noiria_core::scene::Scene;

        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("noiria-v8-tick-drain-test-{ns}"));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("game.json"),
            r#"{"title":"t","id":"noiria-v8-tick-drain-test","main":"main.ts"}"#,
        )?;
        std::fs::write(
            dir.join("main.ts"),
            r#"
const ops = globalThis.Deno.core.ops;
const id = ops.op_layer_new("image");
ops.op_log("info", "armed");
await ops.op_wait(16);
const next = ops.op_wait(16);
await ops.op_wait(0);
ops.op_log("info", "before-scene-gate");
ops.op_layer_set_props(id, "fore", { x: 1 });
ops.op_log("info", "continued");
await next;
ops.op_quit();
"#,
        )?;

        let (game_dir, config) = noiria_core::assets::GameDir::open(dir.clone())?;
        let scene: SharedScene = Arc::new(Mutex::new(Scene::new(64, 64)));
        let (_input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
        let (user_tx, user_rx) = unbounded();
        let (audio_tx, audio_rx) = unbounded();
        drop(audio_rx);

        let start = Instant::now();
        let clock = Clock::new_virtual(start);
        let tick_sync = TickSync::new();
        let pending_waits = PendingWaits::new();
        let op_log: engine_state::SharedOpLog = Arc::new(Mutex::new(Vec::new()));
        let ctx = EngineCtx {
            scene: scene.clone(),
            proxy: WakeSink::Channel(user_tx),
            input_rx,
            trans_notify: Arc::new(Notify::new()),
            audio_tx,
            audio_shared: Arc::new(AudioShared::default()),
            game_dir,
            config,
            start,
            clock: clock.clone(),
            input_sync: InputSync::new(),
            tick_sync: tick_sync.clone(),
            pending_waits: pending_waits.clone(),
            op_log: Some(op_log.clone()),
        };
        let handle = spawn(ctx);
        let deadline = Instant::now() + Duration::from_secs(10);
        let wait_until = |predicate: &dyn Fn() -> bool, message: &str| {
            while !predicate() {
                assert!(Instant::now() < deadline, "{message}");
                std::thread::sleep(Duration::from_millis(1));
            }
        };

        wait_until(
            &|| {
                pending_waits.get() == 1
                    && op_log
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|line| line == "log info armed")
            },
            "最初の仮想 wait が登録されませんでした",
        );

        let scene_guard = scene.lock().unwrap();
        let first_generation = tick_sync.request_for(1);
        clock.advance(Duration::from_millis(16));
        wait_until(
            &|| {
                pending_waits.get() == 1
                    && op_log
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|line| line == "log info before-scene-gate")
            },
            "wait 継続がレイヤ更新へ到達しませんでした",
        );
        assert!(
            tick_sync.completed_generation() < first_generation,
            "JavaScript 継続の実行中に tick barrier が開いています"
        );
        drop(scene_guard);

        wait_until(
            &|| tick_sync.completed_generation() >= first_generation,
            "JavaScript 継続の drain 後も tick barrier が開きませんでした",
        );
        assert!(
            op_log
                .lock()
                .unwrap()
                .iter()
                .any(|line| line == "log info continued"),
            "barrier 完了前に JavaScript 継続が完走していません"
        );

        wait_until(
            &|| pending_waits.get() == 1,
            "継続内で作成した次の仮想 wait が登録されませんでした",
        );
        let second_generation = tick_sync.request_for(1);
        clock.advance(Duration::from_millis(16));
        wait_until(
            &|| tick_sync.completed_generation() >= second_generation,
            "2 番目の仮想 wait が完了しませんでした",
        );
        wait_until(
            &|| handle.is_finished(),
            "スクリプトスレッドが終了しませんでした",
        );
        handle.join().unwrap();
        assert!(
            matches!(
                user_rx
                    .try_iter()
                    .filter(|event| matches!(event, &UserEvent::Quit))
                    .count(),
                1..=2
            ),
            "Quit が通知されませんでした"
        );

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }
}
