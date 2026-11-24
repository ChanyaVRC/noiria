//! エンジン非依存の ops 本体ロジック。
//!
//! V8 (`deno_core` ops, `ops_*.rs`) と tsint (`tsint_engine.rs`) の両方から
//! 呼ばれる。`EngineState` はスクリプトスレッドが 1 個だけ持つ可変状態
//! （シーン・オーディオ送信路・ゲームディレクトリ・テキスト/動画の内部状態
//! 等）をまとめたもので、各操作はここのメソッドとして実装する。
//! V8 側は #[op2] な薄いラッパから、tsint 側はネイティブ関数から、それぞれ
//! このメソッド群を呼ぶだけにして、挙動をふたつのエンジン間で一致させる。

use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use noiria_core::assets::{save_png, GameConfig, GameDir};
use noiria_core::audio::{channel_index, AudioCmd, AudioShared, FadeThen};
use noiria_core::compose::{compose_frame, ComposeBufs};
use noiria_core::pixmap::{checked_dims, Pixmap, ScrollDir};
use noiria_core::savedata::{self, SlotInfo};
use noiria_core::scene::{
    natural_spline_segments, CropTween, Layer, LayerKind, Page, Scene, TextContent, TextReveal,
    Tween, Waypoint,
};
use noiria_core::text::{GlyphRect, TextSegment, TextStyle};
use noiria_core::transition::{TransMethod, Transition};
use tokio::sync::Notify;

use crate::bridge::{
    InputSync, LockExt, PendingWaits, SharedScene, TickSync, UserEvent, WakeSink,
};
use crate::clock::Clock;
use crate::script::ui_cache::{
    load_image_cached, load_image_checked, load_rule_cached, load_scaled_cached, load_thumb_cached,
    panel_key, Prefetcher, UiCaches,
};

/// エンジン非依存のエラー型。V8 側は `JsErrorBox::generic`、tsint 側は
/// `vm.make_error("Error", ..)` にそれぞれ変換する。
pub(crate) type EngineError = String;

/// タグ振る舞いテストハーネス専用の ops 呼び出しログ（`crates/noiria-app/src/script/tag_harness.rs`）。
/// `Arc<Mutex<..>>` にしているのは V8/tsint いずれの `spawn()` も `EngineCtx` を別スレッドへ
/// `move` するため（`Rc<RefCell<..>>` は `!Send` で `spawn` を壊す）。本番経路
/// （`main.rs`）は常に `None` を渡すため、通常プレイでは 1 バイトも書き込まれない
/// （`docs/implementation-notes.md`「2026-07-11 タグ振る舞いテストスイート」参照）。
pub(crate) type SharedOpLog = Arc<Mutex<Vec<String>>>;

mod args;
#[cfg(test)]
mod tests;
mod video;

pub(crate) use args::*;
pub(crate) use video::*;

pub(super) fn parse_page(page: &str) -> Result<Page, EngineError> {
    match page {
        "fore" => Ok(Page::Fore),
        "back" => Ok(Page::Back),
        _ => Err(format!("不明なページ: {page}")),
    }
}

fn ch_of(name: &str) -> Result<usize, EngineError> {
    channel_index(name).ok_or_else(|| format!("不明なチャンネル: {name}"))
}

/// ゲームディレクトリの `fonts/` 配下から `.ttf`/`.ttc` を読み込む（W1: ゲームローカル
/// フォント）。`fonts/` が存在しない/空でもエラーにせず空 Vec を返す（同梱は任意）。
/// 個別ファイルの読み込み失敗はそのファイルだけ warn ログを出してスキップする
/// （システムフォント走査の「1 ファイル失敗でも全体は継続する」方針と揃える）。
fn load_game_font_bytes(game_dir: &GameDir) -> Vec<Vec<u8>> {
    let names = match game_dir.list_dir("fonts") {
        Ok(names) => names,
        Err(e) => {
            log::warn!("fonts/ の列挙に失敗しました: {e:#}");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for name in names {
        let is_font = name
            .rsplit('.')
            .next()
            .map(|ext| {
                let ext = ext.to_ascii_lowercase();
                ext == "ttf" || ext == "ttc"
            })
            .unwrap_or(false);
        if !is_font {
            if name.to_ascii_lowercase().ends_with(".otf") {
                log::error!(
                    "fonts/{name} は読み込めません（OpenType/CFF は非対応。.ttf/.ttc のみ）"
                );
            }
            continue;
        }
        match game_dir.read_bytes(&format!("fonts/{name}")) {
            Ok(bytes) => out.push(bytes),
            Err(e) => log::warn!("ゲームフォントが読めません ({name}): {e:#}"),
        }
    }
    out
}

/// スクリプトスレッドが 1 個だけ持つ、エンジン非依存の可変状態。
pub(crate) struct EngineState {
    pub(crate) scene: SharedScene,
    pub(crate) wake: WakeSink,
    /// V8 版 `op_trans_wait`（`ops_trans.rs`）専用。tsint 版は自前のポーリングループ
    /// （`tsint_engine::PendingOps::trans`）で待つためこのフィールドを読まない
    /// （engine-v8 を無効化したビルドでは未使用になるので dead_code を明示的に許可する）。
    #[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
    pub(crate) trans_notify: Arc<Notify>,
    pub(crate) audio_tx: Sender<AudioCmd>,
    pub(crate) audio_shared: Arc<AudioShared>,
    pub(crate) game_dir: GameDir,
    pub(crate) config: GameConfig,
    pub(crate) start: Instant,
    /// 現在時刻の供給源。アニメーション（tween/transition/quake/reveal）の
    /// 開始時刻をここから記録することで、ヘッドレス実行時は headless.rs の
    /// フレームループが読む値と一致させる（`crate::clock` 参照）。
    pub(crate) clock: Clock,
    /// ヘッドレス専用のロックステップ同期カウンタ（`crate::bridge::InputSync` 参照）。
    /// `op_input_next_event`（V8/tsint 両方）が呼ばれた時点で ack する。windowed 経路では
    /// 誰も観測しないので挙動に影響しない。
    pub(crate) input_sync: InputSync,
    /// ヘッドレスの tick ドレインバリア用カウンタ（`crate::bridge::TickSync` 参照）。
    /// tsint は `drive()`、V8 は `op_wait` と外側の event-loop drain から完了を報告する。
    /// windowed 経路では誰も完了世代を待たないので挙動に影響しない。
    pub(crate) tick_sync: TickSync,
    /// `op_wait`（V8 版）の仮想 in-flight 数（`crate::bridge::PendingWaits` 参照）。
    /// headless.rs がこれを読んで V8 の tick ドレインバリアの要否を判定する。
    /// tsint/windowed 経路では誰も書かない（engine-v8 未使用ビルドでは未読になるため
    /// dead_code を明示的に許可する）。
    #[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
    pub(crate) pending_waits: PendingWaits,
    pub(crate) texts: Texts,
    pub(crate) videos: Videos,
    /// タグ振る舞いテストハーネス専用（既定 `None`）。`Some` の場合のみ `log_op` が
    /// 呼び出し列を蓄積する。
    pub(crate) op_log: Option<SharedOpLog>,
    /// `thumb_capture` が縮小して保持する直近のスクリーンショット（`thumb_save` が書き出す）。
    /// メニューを開く直前の画面を取っておき、メニュー上でのセーブにメニューが写り込まないようにする。
    pending_thumb: Mutex<Option<Pixmap>>,
    /// モダン UI のパネル・サムネイル・縮小画像の描画結果（開くたびに塗り直し/復号し直さない）。
    ui_cache: Arc<Mutex<UiCaches>>,
    prefetcher: Prefetcher,
}

impl EngineState {
    /// V8/tsint 両エンジンの起動経路で共通に使う構築点（W1）。ここでゲームローカル
    /// フォント（`fonts/*.ttf|ttc`）を `game_dir` から読み込んで `Texts::engine` へ
    /// 注入する。フォントが 1 つも無い/読めないゲームでは何もせず、従来どおり
    /// システムフォントのみで動く（Windows の既存挙動は不変）。
    pub(crate) fn from_ctx(
        ctx: super::EngineCtx,
    ) -> (Self, tokio::sync::mpsc::UnboundedReceiver<crate::bridge::InputEvent>) {
        let mut texts = Texts::new();
        texts
            .engine
            .load_game_fonts(load_game_font_bytes(&ctx.game_dir));
        let ui_cache = Arc::new(Mutex::new(UiCaches::new()));
        let prefetcher = Prefetcher::new(
            ui_cache.clone(),
            ctx.game_dir.clone(),
            ctx.config.id.clone(),
        );
        let state = Self {
            scene: ctx.scene,
            wake: ctx.proxy,
            trans_notify: ctx.trans_notify,
            audio_tx: ctx.audio_tx,
            audio_shared: ctx.audio_shared,
            game_dir: ctx.game_dir,
            config: ctx.config,
            start: ctx.start,
            clock: ctx.clock,
            input_sync: ctx.input_sync,
            tick_sync: ctx.tick_sync,
            pending_waits: ctx.pending_waits,
            texts,
            videos: Videos::new(),
            op_log: ctx.op_log,
            pending_thumb: Mutex::new(None),
            ui_cache,
            prefetcher,
        };
        (state, ctx.input_rx)
    }

    // ---- 定型処理 ----

    /// タグ振る舞いテストハーネス用の ops 呼び出し記録。`op_log` が `None`（本番既定）の
    /// 間は何もしない。高頻度なポーリング系 op（`text_is_done`/`audio_is_playing`/
    /// `layer_moving`/`trans_active` 等）には意図的に呼ばない（`wl`/`wm`/`wt` の
    /// ポーリングでログが埋もれるのを避けるため）。
    fn log_op(&self, line: impl Into<String>) {
        if let Some(log) = &self.op_log {
            log.lock_unpoisoned().push(line.into());
        }
    }

    fn with_layer<R>(
        &self,
        page: &str,
        id: u32,
        f: impl FnOnce(&mut Layer) -> R,
    ) -> Result<R, EngineError> {
        let page = parse_page(page)?;
        let result = {
            let mut scene = self.scene.lock_unpoisoned();
            let layer = scene
                .layer_mut(page, id)
                .ok_or_else(|| format!("レイヤが存在しません: {id}"))?;
            let r = f(layer);
            scene.bump();
            r
        };
        self.wake.send(UserEvent::Redraw);
        Ok(result)
    }

    fn with_scene<R>(&self, f: impl FnOnce(&mut Scene) -> R) -> R {
        let result = {
            let mut scene = self.scene.lock_unpoisoned();
            let r = f(&mut scene);
            scene.bump();
            r
        };
        self.wake.send(UserEvent::Redraw);
        result
    }

    // ---- レイヤ ----

    pub(crate) fn layer_new(&self, kind: &str) -> Result<u32, EngineError> {
        let kind_enum = match kind {
            "image" => LayerKind::Image,
            "text" => LayerKind::Text,
            _ => return Err(format!("不明なレイヤ種別: {kind}")),
        };
        let id = self.scene.lock_unpoisoned().alloc_layer(kind_enum);
        self.log_op(format!("layer_new {kind} -> {id}"));
        Ok(id)
    }

    pub(crate) fn layer_free(&self, id: u32) {
        self.log_op(format!("layer_free {id}"));
        self.with_scene(|scene| scene.free_layer(id));
    }

    pub(crate) fn layer_load_image(
        &self,
        id: u32,
        page: &str,
        path: String,
    ) -> Result<(u32, u32), EngineError> {
        self.log_op(format!("layer_load_image {id} {page} {path}"));
        let pixmap = load_image_cached(&self.ui_cache, &self.game_dir, &path)?;
        let size = (pixmap.width, pixmap.height);
        self.with_layer(page, id, |layer| {
            layer.pixmap = Some(pixmap);
            layer.image_path = Some(path);
        })?;
        Ok(size)
    }

    /// [image]（W5 拡張）: `layer_load_image` の拡張版。clip= で部分矩形を抽出し、
    /// flipud=/fliplr=/grayscale= をロード後に適用してからレイヤへ設定する
    /// （`layer_blit_image` と異なり既存内容を消して置き換える）。
    pub(crate) fn layer_load_image_ex(
        &self,
        id: u32,
        page: &str,
        path: String,
        opts: ImageLoadOpts,
    ) -> Result<(u32, u32), EngineError> {
        self.log_op(format!(
            "layer_load_image_ex {id} {page} {path} clip={:?} flipH={} flipV={} gray={}",
            opts.clip, opts.flip_h, opts.flip_v, opts.grayscale
        ));
        let base = load_image_cached(&self.ui_cache, &self.game_dir, &path)?;
        let pixmap = if opts.clip.is_none() && !opts.flip_h && !opts.flip_v && !opts.grayscale {
            base
        } else {
            let mut cur: Cow<'_, Pixmap> = Cow::Borrowed(base.as_ref());
            if let Some(c) = opts.clip {
                checked_dims(c.w, c.h)?;
                cur = Cow::Owned(cur.extract_region(c.x, c.y, c.w, c.h));
            }
            if opts.flip_h {
                cur = Cow::Owned(cur.flip_h());
            }
            if opts.flip_v {
                cur = Cow::Owned(cur.flip_v());
            }
            if opts.grayscale {
                cur = Cow::Owned(cur.grayscale());
            }
            Arc::new(cur.into_owned())
        };
        let size = (pixmap.width, pixmap.height);
        self.with_layer(page, id, |layer| {
            layer.pixmap = Some(pixmap);
            layer.image_path = Some(path);
        })?;
        Ok(size)
    }

    pub(crate) fn layer_clear_image(&self, id: u32, page: &str) -> Result<(), EngineError> {
        self.log_op(format!("layer_clear_image {id} {page}"));
        self.with_layer(page, id, |layer| {
            layer.pixmap = None;
            layer.image_path = None;
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn layer_fill(
        &self,
        id: u32,
        page: &str,
        w: u32,
        h: u32,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    ) -> Result<(), EngineError> {
        self.log_op(format!(
            "layer_fill {id} {page} {w}x{h} rgba=({r},{g},{b},{a})"
        ));
        let mut p = Pixmap::try_new(w, h)?;
        p.fill([r, g, b, a]);
        let p = Arc::new(p);
        self.with_layer(page, id, |layer| {
            layer.pixmap = Some(p);
            layer.image_path = None;
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn layer_msgbox(
        &self,
        id: u32,
        page: &str,
        w: u32,
        h: u32,
        radius: u32,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    ) -> Result<(), EngineError> {
        self.log_op(format!("layer_msgbox {id} {page} {w}x{h}"));
        let mut p = Pixmap::try_new(w, h)?;
        p.fill_rounded_rect(0, 0, w, h, radius, [r, g, b, a]);
        let p = Arc::new(p);
        self.with_layer(page, id, |layer| {
            layer.pixmap = Some(p);
            layer.image_path = None;
        })
    }

    /// 縦/横グラデーション・角丸・枠線・ソフトシャドウ付きのパネルをレイヤの絵にする（モダン UI 用）。
    /// `spec` は `noiria_core::uiraster::PanelSpec::from_slice` の数値列。出力は (w+2*pad)x(h+2*pad)
    /// で、パネル本体は (pad, pad) が左上（呼び出し側がレイヤ位置を pad だけ引く）。
    pub(crate) fn layer_panel(&self, id: u32, page: &str, spec_raw: &[f64]) -> Result<(), EngineError> {
        let spec = noiria_core::uiraster::PanelSpec::from_slice(spec_raw)
            .ok_or_else(|| "layer_panel: 数値列が短すぎます".to_string())?;
        self.log_op(format!(
            "layer_panel {id} {page} {}x{} pad={}",
            spec.w, spec.h, spec.pad
        ));
        let key = panel_key(spec_raw);
        let cached = self.ui_cache.lock_unpoisoned().panels.get(&key);
        let p = match cached {
            Some(p) => p,
            None => {
                let p = Arc::new(noiria_core::uiraster::render_panel(&spec));
                self.ui_cache.lock_unpoisoned().panels.insert(key, p.clone());
                p
            }
        };
        self.with_layer(page, id, |layer| {
            layer.pixmap = Some(p);
            layer.image_path = None;
        })
    }

    /// 画像を読み、面積平均で `w`x`h` へ拡縮してレイヤへ設定する（ギャラリーのサムネイル用）。
    pub(crate) fn layer_load_image_scaled(
        &self,
        id: u32,
        page: &str,
        path: String,
        w: u32,
        h: u32,
    ) -> Result<(u32, u32), EngineError> {
        self.log_op(format!("layer_load_image_scaled {id} {page} {path} {w}x{h}"));
        if w == 0 || h == 0 {
            return Err("layer_load_image_scaled: 寸法が 0 です".to_string());
        }
        checked_dims(w, h)?;
        let pixmap = load_scaled_cached(&self.ui_cache, &self.game_dir, &path, w, h)?;
        self.with_layer(page, id, |layer| {
            layer.pixmap = Some(pixmap);
            layer.image_path = Some(path);
        })?;
        Ok((w, h))
    }

    pub(crate) fn layer_set_props(
        &self,
        id: u32,
        page: &str,
        props: LayerProps,
    ) -> Result<(), EngineError> {
        self.log_op(format!("layer_set_props {id} {page} {props:?}"));
        self.with_layer(page, id, |layer| {
            if let Some(x) = props.x {
                layer.x = x;
            }
            if let Some(y) = props.y {
                layer.y = y;
            }
            if let Some(z) = props.z {
                layer.z = z;
            }
            if let Some(o) = props.opacity {
                layer.opacity = o;
            }
            if let Some(v) = props.visible {
                layer.visible = v;
            }
        })
    }

    /// KAG の [move]: 現在の実効値 (進行中の tween があればその時点の値) を
    /// from として、指定された to へ向けた tween を開始する。省略フィールドは
    /// 現在値を維持する（動かさない）。
    pub(crate) fn layer_move(&self, id: u32, page: &str, args: MoveArgs) -> Result<(), EngineError> {
        self.log_op(format!("layer_move {id} {page} {args:?}"));
        let now = self.clock.now();
        self.with_layer(page, id, |layer| {
            let (from_x, from_y, from_opacity) = match &layer.tween {
                Some(t) => (t.current_x(now), t.current_y(now), t.current_opacity(now)),
                None => (layer.x, layer.y, layer.opacity),
            };
            layer.tween = Some(Tween {
                from_x,
                from_y,
                from_opacity,
                to_x: args.to_x.unwrap_or(from_x),
                to_y: args.to_y.unwrap_or(from_y),
                to_opacity: args.to_opacity.unwrap_or(from_opacity),
                start: now,
                duration: Duration::from_millis(args.time_ms),
                accel: args.accel,
                spline: None,
            });
            layer.tween_queue.clear();
        })
    }

    /// 画像の「見える幅」（左端から何画素ぶん合成するか）を即座に決める（進行中の `layer_crop_move` は止まる）。
    pub(crate) fn layer_set_crop(&self, id: u32, page: &str, w: u32) -> Result<(), EngineError> {
        self.log_op(format!("layer_set_crop {id} {page} {w}"));
        self.with_layer(page, id, |layer| {
            layer.crop_w = Some(w);
            layer.crop_tween = None;
        })
    }

    /// 画像の「見える幅」（左端から何画素ぶん合成するか）を、今の値から `to` へ tween する
    /// （アンダーラインが左から伸びる等のモダン UI 用。位置/不透明度の `layer_move` とは独立に動く）。
    pub(crate) fn layer_crop_move(
        &self,
        id: u32,
        page: &str,
        to: u32,
        time_ms: u64,
        accel: f32,
    ) -> Result<(), EngineError> {
        self.log_op(format!("layer_crop_move {id} {page} to={to} time_ms={time_ms} accel={accel}"));
        let now = self.clock.now();
        self.with_layer(page, id, |layer| {
            let full = layer.pixmap.as_ref().map(|p| p.width).unwrap_or(0);
            let from = layer.effective_crop(now).unwrap_or(full);
            layer.crop_w = Some(from);
            layer.crop_tween = Some(CropTween {
                from,
                to,
                start: now,
                duration: Duration::from_millis(time_ms),
                accel,
            });
        })
    }

    pub(crate) fn layer_move_cancel(&self, id: u32, page: &str) -> Result<(), EngineError> {
        self.log_op(format!("layer_move_cancel {id} {page}"));
        self.with_layer(page, id, |layer| {
            layer.tween = None;
            layer.tween_queue.clear();
        })
    }

    /// 全レイヤの tween を即座に完了させる（スキップモード用）。[stopmove]（W5）もこれを
    /// そのまま流用する（全レイヤの move を目標値で即完了させるという効果が一致するため）。
    pub(crate) fn layer_move_skip(&self) {
        self.log_op("layer_move_skip");
        self.with_scene(|scene| scene.skip_all_tweens());
    }

    /// [move path=]（W5/B12）: 複数区間の move。`points` は区間ごとの `MoveArgs`
    /// （各区間の toX/toY/toOpacity/timeMs/accel）。区間の toX/toY/toOpacity を省略した場合は
    /// 直前の区間の値（先頭区間は現在の実効値）を引き継ぐ。第一区間は現在値からの通常の
    /// `layer_move` と同じ開始をし、残りは `tween_queue` へ積んで `Scene::settle_finished_tweens`
    /// が順につなげる。`spline=true` では現在位置を先頭点とする自然三次スプラインを使う。
    pub(crate) fn layer_move_path(
        &self,
        id: u32,
        page: &str,
        points: Vec<MoveArgs>,
        spline: bool,
    ) -> Result<(), EngineError> {
        self.layer_move_path_inner(id, page, points, spline, None)
    }

    pub(crate) fn layer_move_path_by_distance(
        &self,
        id: u32,
        page: &str,
        points: Vec<MoveArgs>,
        spline: bool,
        total_time_ms: f64,
    ) -> Result<(), EngineError> {
        if !total_time_ms.is_finite() || total_time_ms < 0.0 || total_time_ms > u32::MAX as f64 {
            return Err("[move] time= は有限な非負値で指定してください".to_string());
        }
        self.layer_move_path_inner(id, page, points, spline, Some(total_time_ms))
    }

    fn layer_move_path_inner(
        &self,
        id: u32,
        page: &str,
        points: Vec<MoveArgs>,
        spline: bool,
        total_time_ms: Option<f64>,
    ) -> Result<(), EngineError> {
        if points.is_empty() {
            return Err("[move] path= には少なくとも 1 点が必要です".to_string());
        }
        if spline && points.len() < 2 {
            return Err("[move] spline=true の path= には少なくとも 2 点が必要です".to_string());
        }
        match total_time_ms {
            Some(total) => self.log_op(format!(
                "layer_move_path_distance {id} {page} legs={} spline={spline} totalMs={total}",
                points.len()
            )),
            None => self.log_op(format!(
                "layer_move_path {id} {page} legs={} spline={spline}",
                points.len()
            )),
        }
        let now = self.clock.now();
        self.with_layer(page, id, |layer| {
            let (from_x, from_y, from_opacity) = match &layer.tween {
                Some(t) => (t.current_x(now), t.current_y(now), t.current_opacity(now)),
                None => (layer.x, layer.y, layer.opacity),
            };
            let (mut px, mut py, mut po) = (from_x, from_y, from_opacity);
            let mut resolved: Vec<Waypoint> = Vec::with_capacity(points.len());
            for p in &points {
                let x = p.to_x.unwrap_or(px);
                let y = p.to_y.unwrap_or(py);
                let o = p.to_opacity.unwrap_or(po);
                resolved.push(Waypoint {
                    x,
                    y,
                    opacity: o,
                    duration: Duration::from_millis(p.time_ms),
                    accel: p.accel,
                    spline: None,
                });
                px = x;
                py = y;
                po = o;
            }
            if let Some(total) = total_time_ms {
                let mut distances = Vec::with_capacity(resolved.len());
                let (mut x, mut y) = (from_x, from_y);
                let mut distance_sum = 0.0;
                for waypoint in &resolved {
                    let dx = f64::from(waypoint.x) - f64::from(x);
                    let dy = f64::from(waypoint.y) - f64::from(y);
                    let distance = (dx * dx + dy * dy).sqrt();
                    distances.push(distance);
                    distance_sum += distance;
                    x = waypoint.x;
                    y = waypoint.y;
                }
                let equal_share = 1.0 / resolved.len() as f64;
                for (waypoint, distance) in resolved.iter_mut().zip(distances) {
                    let share = if distance_sum > 0.0 {
                        distance / distance_sum
                    } else {
                        equal_share
                    };
                    waypoint.duration = Duration::from_secs_f64(total * share / 1000.0);
                }
            }
            if spline {
                let mut positions = Vec::with_capacity(resolved.len() + 1);
                positions.push((from_x, from_y));
                positions.extend(resolved.iter().map(|wp| (wp.x, wp.y)));
                let segments = natural_spline_segments(&positions);
                for (waypoint, segment) in resolved.iter_mut().zip(segments) {
                    waypoint.spline = Some(segment);
                }
            }
            let mut legs = resolved.into_iter();
            let first = legs.next().expect("points は非空を確認済み");
            layer.tween = Some(Tween {
                from_x,
                from_y,
                from_opacity,
                to_x: first.x,
                to_y: first.y,
                to_opacity: first.opacity,
                start: now,
                duration: first.duration,
                accel: first.accel,
                spline: first.spline,
            });
            layer.tween_queue = legs.collect();
        })
    }

    /// いずれかの fore レイヤで tween が進行中か（[wm] のポーリングに使う）。
    pub(crate) fn layer_moving(&self) -> bool {
        let now = self.clock.now();
        let scene = self.scene.lock_unpoisoned();
        scene.fore.values().any(|l| {
            l.tween.as_ref().is_some_and(|t| !t.is_finished(now)) || !l.tween_queue.is_empty()
        })
    }

    /// KAG の backlay。id < 0 で全レイヤ。
    pub(crate) fn backlay(&self, id: i32) {
        self.log_op(format!("backlay {id}"));
        self.with_scene(|scene| scene.backlay(if id < 0 { None } else { Some(id as u32) }));
    }

    /// KAG の [copylay]（W3、W5 で `copy_position` を追加）: 異なるレイヤ間で内容をコピーする。
    /// `copy_position=false` はコピー先の x/y を変えない（メッセージレイヤ宛の既知バグ修正、
    /// `Scene::copy_layer` のコメント参照）。
    pub(crate) fn layer_copy(
        &self,
        src_id: u32,
        src_page: &str,
        dst_id: u32,
        dst_page: &str,
        copy_position: bool,
    ) -> Result<(), EngineError> {
        self.log_op(format!(
            "layer_copy {src_id}/{src_page} -> {dst_id}/{dst_page} copy_position={copy_position}"
        ));
        let sp = parse_page(src_page)?;
        let dp = parse_page(dst_page)?;
        self.with_scene(|scene| scene.copy_layer(sp, src_id, dp, dst_id, copy_position));
        Ok(())
    }

    /// `layer_blit_image`（W4）: 画像ファイル（の clip= 部分矩形、省略時は全体）を
    /// 既存レイヤへ (dx, dy) を左上として over 合成する。`layer_load_image` と異なり
    /// 既存のピクセル内容を消さずに重ね描きする（[button] の 3 状態画像切替、W5 の
    /// [pimage] と共用する設計）。レイヤにまだ pixmap が無い場合は、blit するぶんの
    /// 透明キャンバスを新規に確保してから合成する（呼び出し側が事前に [layer.fill] 等で
    /// キャンバスを確保しておくのが基本だが、最小限の自動確保もしておく）。
    pub(crate) fn layer_blit_image(
        &self,
        id: u32,
        page: &str,
        path: &str,
        dx: i32,
        dy: i32,
        clip: Option<BlitClip>,
    ) -> Result<(), EngineError> {
        self.log_op(format!(
            "layer_blit_image {id} {page} {path} dx={dx} dy={dy} clip={clip:?}"
        ));
        let src = load_image_cached(&self.ui_cache, &self.game_dir, path)?;
        let region: Cow<'_, Pixmap> = match clip {
            Some(c) => {
                checked_dims(c.w, c.h)?;
                Cow::Owned(src.extract_region(c.x, c.y, c.w, c.h))
            }
            None => Cow::Borrowed(src.as_ref()),
        };
        self.with_layer(page, id, |layer| {
            let pixmap = layer
                .pixmap
                .get_or_insert_with(|| Arc::new(Pixmap::new(region.width, region.height)));
            Arc::make_mut(pixmap).blit_over(&region, dx, dy, 255);
            layer.image_path = None;
        })
    }

    /// 画面揺れ。
    pub(crate) fn quake(&self, ax: i32, ay: i32, ms: u32) {
        self.log_op(format!("quake ax={ax} ay={ay} ms={ms}"));
        let now = self.clock.now();
        self.with_scene(|scene| {
            scene.quake = Some((ax, ay, now, ms));
        });
    }

    /// [stopquake]（W5）: 画面揺れを即座に停止する。
    pub(crate) fn quake_stop(&self) {
        self.log_op("quake_stop");
        self.with_scene(|scene| {
            scene.quake = None;
        });
    }

    // ---- テキスト ----

    pub(crate) fn text_style(&mut self, id: u32, style: TextStyle) {
        self.log_op(format!(
            "text_style {id} align={:?} line_height={} font_family={}",
            style.align, style.line_height, style.font_family
        ));
        self.texts.styles.insert(id, style);
    }

    fn ensure_inline_images(&mut self, segments: &[TextSegment]) {
        // [graph]（インライン画像, W3）: text.rs は assets への依存を増やさない設計にしたため
        // （`docs/implementation-notes.md` 2026-07-11 W3 参照）、実ファイル I/O はここで行い、
        // ロード済みの Pixmap を TextEngine の画像キャッシュへ渡す。
        for seg in segments {
            if let Some(path) = &seg.inline_image {
                if !self.texts.engine.has_image(path) {
                    match load_image_checked(&self.game_dir, path) {
                        Ok(pixmap) => self.texts.engine.cache_image(path.clone(), pixmap),
                        Err(e) => log::warn!("[graph] 画像の読み込みに失敗しました: {path}: {e:#}"),
                    }
                }
            }
        }
    }

    pub(crate) fn text_print(
        &mut self,
        id: u32,
        segments: Vec<TextSegment>,
        append: bool,
        cps: f64,
    ) -> Result<PrintResult, EngineError> {
        self.log_op(format!(
            "text_print {id} append={append} cps={cps} segs={segments:?}"
        ));

        let (style, all, from_glyph) = {
            let texts = &mut self.texts;
            let style = texts.styles.entry(id).or_default().clone();
            let entry = texts.segments.entry(id).or_default();

            // 追記時: 前回レイアウトの総グリフ数をキャッシュから使う
            let from_glyph = if append {
                texts.glyph_counts.get(&id).copied().unwrap_or(0)
            } else {
                entry.clear();
                0
            };
            entry.extend(segments);
            (style, entry.clone(), from_glyph)
        };
        self.ensure_inline_images(&all);
        let raster = self.texts.engine.raster_segments(&all, &style);
        self.texts.glyph_counts.insert(id, raster.glyphs.len());

        let total = raster.glyphs.len();
        let added = total.saturating_sub(from_glyph);
        let raster = Arc::new(raster);
        let reveal_start = if cps > 0.0 {
            Some(self.clock.now())
        } else {
            None
        };
        self.with_layer("fore", id, move |layer| {
            layer.text = Some(TextContent {
                raster,
                reveal: TextReveal {
                    from_glyph,
                    start: reveal_start,
                    cps: cps as f32,
                },
            });
        })?;
        Ok(PrintResult { total, added })
    }

    pub(crate) fn text_reflow(&mut self, id: u32) -> Result<(), EngineError> {
        self.log_op(format!("text_reflow {id}"));
        let style = self.texts.styles.entry(id).or_default().clone();
        let segments = self.texts.segments.entry(id).or_default().clone();
        self.ensure_inline_images(&segments);
        let raster = self.texts.engine.raster_segments(&segments, &style);
        self.texts.glyph_counts.insert(id, raster.glyphs.len());
        let raster = Arc::new(raster);
        self.with_layer("fore", id, move |layer| match &mut layer.text {
            Some(text) => text.raster = raster,
            None => {
                layer.text = Some(TextContent {
                    raster,
                    reveal: TextReveal {
                        from_glyph: 0,
                        start: None,
                        cps: 0.0,
                    },
                });
            }
        })
    }

    /// リベールを即時完了させる。
    pub(crate) fn text_complete(&self, id: u32) {
        self.log_op(format!("text_complete {id}"));
        let _ = self.with_layer("fore", id, |layer| {
            if let Some(text) = &mut layer.text {
                text.reveal.start = None;
            }
        });
    }

    /// リベールが完了しているか。
    pub(crate) fn text_is_done(&self, id: u32) -> bool {
        let scene = self.scene.lock_unpoisoned();
        scene
            .layer(Page::Fore, id)
            .and_then(|l| l.text.as_ref())
            .map(|t| t.reveal.is_done(t.raster.glyphs.len(), self.clock.now()))
            .unwrap_or(true)
    }

    pub(crate) fn text_clear(&mut self, id: u32) {
        self.log_op(format!("text_clear {id}"));
        self.texts.segments.remove(&id);
        self.texts.glyph_counts.remove(&id);
        let _ = self.with_layer("fore", id, |layer| {
            layer.text = None;
        });
    }

    /// グリフのヒット矩形（リンク選択肢・メニュー行判定用）。低頻度（実クリック時のみ）
    /// なので op_log にも記録する（タグ振る舞いテストハーネスがリンクの実座標を知る手段）。
    pub(crate) fn text_rects(&self, id: u32) -> Vec<GlyphRect> {
        let scene = self.scene.lock_unpoisoned();
        let rects = scene
            .layer(Page::Fore, id)
            .and_then(|l| l.text.as_ref())
            .map(|t| t.raster.rects())
            .unwrap_or_default();
        self.log_op(format!("text_rects {id} {rects:?}"));
        rects
    }

    // ---- トランジション ----

    pub(crate) fn trans_begin(&self, args: TransArgs) -> Result<(), EngineError> {
        self.log_op(format!("trans_begin {} {}", args.method, args.time_ms));
        let method = match args.method.as_str() {
            "crossfade" => TransMethod::Crossfade,
            "universal" => {
                let rule_path = args
                    .rule
                    .as_deref()
                    .ok_or_else(|| "universal トランジションには rule が必要です".to_string())?;
                let (scene_w, scene_h) = {
                    let scene = self.scene.lock_unpoisoned();
                    (scene.width, scene.height)
                };
                let rule = load_rule_cached(
                    &self.ui_cache,
                    &self.game_dir,
                    rule_path,
                    scene_w,
                    scene_h,
                )?;
                TransMethod::Universal {
                    rule,
                    vague: args.vague.unwrap_or(64.0),
                }
            }
            "scroll" => {
                let from = match args.from.as_deref() {
                    Some("right") => ScrollDir::Right,
                    Some("top") => ScrollDir::Top,
                    Some("bottom") => ScrollDir::Bottom,
                    _ => ScrollDir::Left, // 既定 left
                };
                TransMethod::Scroll { from }
            }
            other => return Err(format!("不明なトランジション: {other}")),
        };
        {
            let mut scene = self.scene.lock_unpoisoned();
            scene.transition = Some(Transition {
                method,
                start: self.clock.now(),
                duration: Duration::from_millis(args.time_ms),
                skip: false,
            });
            scene.bump();
        }
        self.wake.send(UserEvent::Redraw);
        Ok(())
    }

    pub(crate) fn trans_skip(&self) {
        self.log_op("trans_skip");
        {
            let mut scene = self.scene.lock_unpoisoned();
            if let Some(t) = &mut scene.transition {
                t.skip = true;
            }
            scene.bump();
        }
        self.wake.send(UserEvent::Redraw);
    }

    /// トランジションが進行中か（非同期 `op_trans_wait` のポーリングに使う）。
    pub(crate) fn trans_active(&self) -> bool {
        self.scene.lock_unpoisoned().transition.is_some()
    }

    // ---- オーディオ ----

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn audio_play(
        &self,
        ch: &str,
        path: &str,
        looping: bool,
        volume: f64,
        fade_in_ms: u32,
        start_position_ms: f64,
        start_paused: bool,
    ) -> Result<(), EngineError> {
        if !start_position_ms.is_finite() || start_position_ms < 0.0 {
            return Err("音声の開始位置は有限の非負値で指定してください".to_string());
        }
        let start_position_ms = start_position_ms.round() as u64;
        let ch_index = ch_of(ch)?;
        let request_id = self
            .audio_shared
            .mark_play_requested(ch_index, start_position_ms);
        self.log_op(format!(
            "audio_play {ch} {path} loop={looping} vol={volume} fade={fade_in_ms} start={start_position_ms} paused={start_paused}"
        ));
        // ヘッドレス検証用の実行証跡（音を出せない環境で再生回数・タイミングを確認する唯一の手段）
        log::debug!("[audio] play ch={ch_index:?} path={path} loop={looping}");
        // W5: パスジェイル/ファイル読込はオーディオスレッド側 (`audio::build_source` →
        // `GameDir::read_bytes`) に一本化した。そのため不正な相対パス（`../` 等）は
        // ここでは検出できず、オーディオスレッドの `log::error!` として非同期に出る
        // （以前は `resolve()` をここで呼んで同期的に Err を返していた）。
        // `docs/implementation-notes.md`「2026-07-12 配布対応 W5」参照。
        let sent = self.audio_tx.send(AudioCmd::Play {
            ch: ch_index,
            rel: path.to_string(),
            looping,
            volume: volume as f32,
            fade_in_ms,
            start_position_ms,
            start_paused,
            request_id,
        });
        if sent.is_err() {
            // 音声スレッドが無い（ヘッドレス等）: 誰も確認応答しないので取り下げる。
            self.audio_shared
                .cancel_play_request(ch_index, request_id, start_position_ms);
        }
        Ok(())
    }

    pub(crate) fn audio_stop(&self, ch: &str, fade_out_ms: u32) -> Result<(), EngineError> {
        let ch_index = ch_of(ch)?;
        let request_id = self.audio_shared.mark_stop_requested(ch_index);
        self.log_op(format!("audio_stop {ch} fade={fade_out_ms}"));
        let _ = self.audio_tx.send(AudioCmd::Stop {
            ch: ch_index,
            fade_out_ms,
            request_id,
        });
        Ok(())
    }

    pub(crate) fn audio_volume(&self, ch: &str, volume: f64) -> Result<(), EngineError> {
        self.log_op(format!("audio_volume {ch} vol={volume}"));
        let ch = ch_of(ch)?;
        let _ = self.audio_tx.send(AudioCmd::SetVolume {
            ch,
            volume: volume as f32,
        });
        Ok(())
    }

    /// 再生中、または Play 要求済みでデコード中か（再生終了待ちの判定。デコード中は `is_playing` が偽）。
    #[cfg_attr(not(feature = "engine-tsint"), allow(dead_code))]
    pub(crate) fn audio_busy(&self, ch: usize) -> bool {
        self.audio_shared.is_playing(ch) || self.audio_shared.is_play_pending(ch)
    }

    pub(crate) fn audio_is_playing(&self, ch: &str) -> Result<bool, EngineError> {
        let ch = ch_of(ch)?;
        Ok(self.audio_shared.is_playing(ch))
    }

    pub(crate) fn audio_position(&self, ch: &str) -> Result<u64, EngineError> {
        let ch = ch_of(ch)?;
        Ok(self.audio_shared.position_ms(ch))
    }

    pub(crate) fn audio_pause(&self, ch: &str) -> Result<(), EngineError> {
        self.log_op(format!("audio_pause {ch}"));
        let ch = ch_of(ch)?;
        let _ = self.audio_tx.send(AudioCmd::Pause { ch });
        Ok(())
    }

    pub(crate) fn audio_resume(&self, ch: &str) -> Result<(), EngineError> {
        self.log_op(format!("audio_resume {ch}"));
        let ch = ch_of(ch)?;
        let _ = self.audio_tx.send(AudioCmd::Resume { ch });
        Ok(())
    }

    /// `then`: "none" | "stop" | "pause"（フェード完了時の遷移）。
    pub(crate) fn audio_fade_to(
        &self,
        ch: &str,
        volume: f64,
        ms: u32,
        then: &str,
    ) -> Result<(), EngineError> {
        self.log_op(format!(
            "audio_fade_to {ch} vol={volume} ms={ms} then={then}"
        ));
        let ch = ch_of(ch)?;
        let then = match then {
            "none" => FadeThen::None,
            "stop" => FadeThen::Stop,
            "pause" => FadeThen::Pause,
            other => return Err(format!("不明な then 指定: {other}")),
        };
        let _ = self.audio_tx.send(AudioCmd::FadeTo {
            ch,
            target_vol: volume as f32,
            ms,
            then,
        });
        Ok(())
    }

    pub(crate) fn audio_is_fading(&self, ch: &str) -> Result<bool, EngineError> {
        Ok(self.audio_shared.is_fading(ch_of(ch)?))
    }

    pub(crate) fn audio_is_finished(&self, ch: &str) -> Result<bool, EngineError> {
        Ok(self.audio_shared.is_finished(ch_of(ch)?))
    }

    // ---- システム ----

    /// アプリ起動からのミリ秒。
    pub(crate) fn now(&self) -> f64 {
        self.clock.now().duration_since(self.start).as_secs_f64() * 1000.0
    }

    /// `[eval]`/W2〜W6 の各タグが `api.sys.log(...)` 経由で出す実行証跡ログ。多くの
    /// W2/W4/W6 タグ（bgmopt/pausebgm/button/timeout/tempsave/save/... 等）はレイヤ/オーディオ
    /// ops を一切呼ばずこのログだけが観測可能な効果なので、タグ振る舞いテストハーネスのために
    /// `op_log` にも記録する（本番既定 `None` の間は従来通り `log()` を呼ぶだけ）。
    pub(crate) fn sys_log(&self, level: &str, msg: &str) {
        self.log_op(format!("log {level} {msg}"));
        log(level, msg);
    }

    pub(crate) fn quit(&self) {
        self.log_op("quit");
        self.wake.send(UserEvent::Quit);
    }

    pub(crate) fn set_title(&self, title: String) {
        self.log_op(format!("set_title {title}"));
        self.wake.send(UserEvent::SetTitle(title));
    }

    /// ゲームディレクトリ内のテキストファイル (.ks 等) を読む。
    pub(crate) fn read_text(&self, rel: &str) -> Result<String, EngineError> {
        self.game_dir.read_text(rel).map_err(|e| e.to_string())
    }

    pub(crate) fn file_exists(&self, rel: &str) -> bool {
        self.game_dir.exists(rel)
    }

    pub(crate) fn save_write(&self, slot: &str, json: &str) -> Result<(), EngineError> {
        self.log_op(format!("save_write {slot}"));
        savedata::write_slot(&self.config.id, slot, json).map_err(|e| e.to_string())
    }

    pub(crate) fn save_read(&self, slot: &str) -> Result<Option<String>, EngineError> {
        self.log_op(format!("save_read {slot}"));
        savedata::read_slot(&self.config.id, slot).map_err(|e| e.to_string())
    }

    pub(crate) fn save_list(&self) -> Result<Vec<SlotInfo>, EngineError> {
        self.log_op("save_list");
        savedata::list_slots(&self.config.id).map_err(|e| e.to_string())
    }

    /// [erasebookmark] の真の削除（W7）。存在しないスロットは no-op。
    pub(crate) fn save_delete(&self, slot: &str) -> Result<(), EngineError> {
        self.log_op(format!("save_delete {slot}"));
        self.ui_cache.lock_unpoisoned().forget_slot(slot);
        savedata::delete_slot(&self.config.id, slot).map_err(|e| e.to_string())
    }

    /// シーンのロックはスナップショット（`Scene::clone`、Arc の複製のみ）を取る間だけ握り、確保・合成は
    /// ロックの外で行う（握ったままだと、合成の数十 ms のあいだ描画スレッドと ops が待たされる）。
    fn compose_current(&self) -> Result<Pixmap, EngineError> {
        let (scene, now) = {
            let scene = self.scene.lock_unpoisoned();
            (scene.clone(), self.clock.now())
        };
        let mut bufs = ComposeBufs::new(scene.width, scene.height);
        let mut out = Pixmap::try_new(scene.width, scene.height)?;
        compose_frame(&scene, now, &mut bufs, &mut out);
        Ok(out)
    }

    /// 現在のシーンを合成してセーブディレクトリへ PNG 保存（サムネイル/オートテスト用）。
    pub(crate) fn screenshot(&self, slot: &str) -> Result<(), EngineError> {
        self.log_op(format!("screenshot {slot}"));
        let path = savedata::thumb_path(&self.config.id, slot).map_err(|e| e.to_string())?;
        let out = self.compose_current()?;
        save_png(&out, &path).map_err(|e| e.to_string())
    }

    /// 現在のシーンを合成し、縮小して保持する（メニューを開く直前の画面をサムネイルにするため）。
    /// w/h のどちらか 0 なら規定寸法 (`THUMB_W`x`THUMB_H`)、それ以外はその寸法（モダン UI の大きいカード用）。
    pub(crate) fn thumb_capture_sized(&self, w: u32, h: u32) -> Result<(), EngineError> {
        let (w, h) = if w == 0 || h == 0 {
            (savedata::THUMB_W, savedata::THUMB_H)
        } else {
            (w, h)
        };
        if (w, h) == (savedata::THUMB_W, savedata::THUMB_H) {
            self.log_op("thumb_capture");
        } else {
            self.log_op(format!("thumb_capture {w}x{h}"));
        }
        checked_dims(w, h)?;
        let out = self.compose_current()?;
        let thumb = out.resize_area(w, h);
        *self.pending_thumb.lock_unpoisoned() = Some(thumb);
        Ok(())
    }

    /// 直近の `thumb_capture` の結果を `<slot>.png` へ書く。未取得なら今の画面を取ってから書く。
    pub(crate) fn thumb_save(&self, slot: &str) -> Result<(), EngineError> {
        self.log_op(format!("thumb_save {slot}"));
        if self.pending_thumb.lock_unpoisoned().is_none() {
            self.thumb_capture_sized(0, 0)?;
        }
        let guard = self.pending_thumb.lock_unpoisoned();
        let Some(thumb) = guard.as_ref() else {
            return Err("サムネイルの取得に失敗しました".to_string());
        };
        let r = savedata::write_thumb(&self.config.id, slot, thumb).map_err(|e| e.to_string());
        self.ui_cache.lock_unpoisoned().forget_slot(slot);
        r
    }

    /// セーブ領域の `<slot>.png` をレイヤへ読み込む（`GameDir` ではなくセーブ領域の専用経路。
    /// スロット名は英数字/`_`/`-` のみ）。規定寸法より大きい PNG は収まるよう縮小する。
    /// 戻り値は読み込んだ画像の寸法。無い/壊れていれば Err。
    ///
    /// w/h が 0 以外なら、保存された大きさに関わらずちょうど w x h へ面積平均で拡縮して読む
    /// （モダン UI のカード用。0 は規定寸法に収める従来動作）。
    pub(crate) fn layer_load_save_thumb_sized(
        &self,
        id: u32,
        page: &str,
        slot: &str,
        w: u32,
        h: u32,
    ) -> Result<(u32, u32), EngineError> {
        let sized = w != 0 && h != 0;
        if sized {
            self.log_op(format!("layer_load_save_thumb {id} {page} {slot} {w}x{h}"));
        } else {
            self.log_op(format!("layer_load_save_thumb {id} {page} {slot}"));
        }
        let pixmap = load_thumb_cached(&self.ui_cache, &self.config.id, slot, w, h)?;
        let size = (pixmap.width, pixmap.height);
        self.with_layer(page, id, |layer| {
            layer.pixmap = Some(pixmap);
            layer.image_path = None;
        })?;
        Ok(size)
    }

    /// セーブ一覧のサムネイルを別スレッドで先に復号してキャッシュへ入れておく（初回にメニューを開くときの
    /// 数十 ms を、タイトル画面などの待ち時間へ逃がす）。表示には影響しない。
    pub(crate) fn prefetch_save_thumbs(&self, w: u32, h: u32) {
        self.log_op(format!("prefetch_save_thumbs {w}x{h}"));
        self.prefetcher.thumbs(w, h);
    }

    /// ギャラリーのサムネイル（ゲーム資産の画像を w x h へ縮小したもの）を別スレッドで先に作っておく。
    pub(crate) fn prefetch_images_scaled(&self, paths: Vec<String>, w: u32, h: u32) {
        self.log_op(format!("prefetch_images_scaled {} {w}x{h}", paths.len()));
        self.prefetcher.scaled(paths, w, h);
    }

    /// セーブ一覧に出す保存日時（`YYYY/MM/DD HH:MM`、ローカル時刻）。ヘッドレス（仮想クロック）では
    /// フレームを決定論に保つため固定値を返す。
    pub(crate) fn wall_clock_string(&self) -> String {
        if !self.clock.is_real() {
            return "2026/01/01 12:00".to_string();
        }
        noiria_core::localtime::now_local()
            .map(|t| t.format())
            .unwrap_or_default()
    }

}

/// ログ出力。`OpState`/`EngineState` に依存しない自由関数（旧 `ops_sys::op_log`）。
pub(crate) fn log(level: &str, msg: &str) {
    match level {
        "error" => log::error!("[script] {msg}"),
        "warn" => log::warn!("[script] {msg}"),
        "debug" => log::debug!("[script] {msg}"),
        _ => log::info!("[script] {msg}"),
    }
}

