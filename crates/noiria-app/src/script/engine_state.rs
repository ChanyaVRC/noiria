//! エンジン非依存の ops 本体ロジック。
//!
//! V8 (`deno_core` ops, `ops_*.rs`) と tsint (`tsint_engine.rs`) の両方から
//! 呼ばれる。`EngineState` はスクリプトスレッドが 1 個だけ持つ可変状態
//! （シーン・オーディオ送信路・ゲームディレクトリ・テキスト/動画の内部状態
//! 等）をまとめたもので、各操作はここのメソッドとして実装する。
//! V8 側は #[op2] な薄いラッパから、tsint 側はネイティブ関数から、それぞれ
//! このメソッド群を呼ぶだけにして、挙動をふたつのエンジン間で一致させる。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use noiria_core::assets::{save_png, GameConfig, GameDir};
use noiria_core::audio::{channel_index, AudioCmd, AudioShared, FadeThen};
use noiria_core::codec::video::VideoSource;
use noiria_core::compose::{compose_frame, ComposeBufs};
use noiria_core::pixmap::{checked_dims, Pixmap, ScrollDir};
use noiria_core::savedata::{self, SlotInfo};
use noiria_core::scene::{
    natural_spline_segments, CropTween, Layer, LayerKind, Page, Scene, TextContent, TextReveal,
    Tween, Waypoint,
};
use noiria_core::text::{
    GlyphRect, LocateArgs, TextAlign, TextEngine, TextSegment, TextStyle,
};
use noiria_core::transition::{rule_plane_from_rgba, TransMethod, Transition};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::bridge::{
    InputSync, LockExt, PendingWaits, SharedScene, TickSync, UserEvent, WakeSink,
};
use crate::clock::Clock;
use crate::script::numconv::{
    de_opt_coord, de_opt_i32, de_opt_u32, de_opt_u8, de_time_ms, de_u32, to_u8,
};
use crate::script::ui_cache::{
    load_image_checked, load_scaled_cached, load_thumb_cached, panel_key, Prefetcher, UiCaches,
};

/// エンジン非依存のエラー型。V8 側は `JsErrorBox::generic`、tsint 側は
/// `vm.make_error("Error", ..)` にそれぞれ変換する。
pub type EngineError = String;

/// タグ振る舞いテストハーネス専用の ops 呼び出しログ（`crates/noiria-app/src/script/tag_harness.rs`）。
/// `Arc<Mutex<..>>` にしているのは V8/tsint いずれの `spawn()` も `EngineCtx` を別スレッドへ
/// `move` するため（`Rc<RefCell<..>>` は `!Send` で `spawn` を壊す）。本番経路
/// （`main.rs`）は常に `None` を渡すため、通常プレイでは 1 バイトも書き込まれない
/// （`docs/implementation-notes.md`「2026-07-11 タグ振る舞いテストスイート」参照）。
pub type SharedOpLog = Arc<Mutex<Vec<String>>>;

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct LayerProps {
    #[serde(deserialize_with = "de_opt_coord")]
    pub x: Option<i32>,
    #[serde(deserialize_with = "de_opt_coord")]
    pub y: Option<i32>,
    #[serde(deserialize_with = "de_opt_i32")]
    pub z: Option<i32>,
    #[serde(deserialize_with = "de_opt_u8")]
    pub opacity: Option<u8>,
    pub visible: Option<bool>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MoveArgs {
    #[serde(deserialize_with = "de_opt_coord")]
    pub to_x: Option<i32>,
    #[serde(deserialize_with = "de_opt_coord")]
    pub to_y: Option<i32>,
    #[serde(deserialize_with = "de_opt_u8")]
    pub to_opacity: Option<u8>,
    #[serde(deserialize_with = "de_time_ms")]
    pub time_ms: u64,
    pub accel: f32,
}

/// `layer_blit_image` の clip= 矩形（画像ファイル内の切り出し範囲）。省略時は画像全体。
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct BlitClip {
    #[serde(deserialize_with = "de_u32")]
    pub x: u32,
    #[serde(deserialize_with = "de_u32")]
    pub y: u32,
    #[serde(deserialize_with = "de_u32")]
    pub w: u32,
    #[serde(deserialize_with = "de_u32")]
    pub h: u32,
}

/// [image]（W5 拡張）の clip=/flipud=/fliplr=/grayscale= 属性。`layer_load_image` の拡張版
/// `layer_load_image_ex` が使う。適用順は clip 抽出 → 左右反転 → 上下反転 → グレースケール化。
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImageLoadOpts {
    pub clip: Option<BlitClip>,
    pub flip_h: bool,
    pub flip_v: bool,
    pub grayscale: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransArgs {
    pub method: String,
    #[serde(default, deserialize_with = "de_time_ms")]
    pub time_ms: u64,
    #[serde(default)]
    pub rule: Option<String>,
    #[serde(default)]
    pub vague: Option<f32>,
    /// [trans method=scroll]（W5）: 新ページが入ってくる辺（既定 left）。
    #[serde(default)]
    pub from: Option<String>,
    /// [trans method=scroll]（W5）: stayfore|staynone|staybase を受理するが、描画上は
    /// 区別しない（`Pixmap::scroll_from` のコメント参照）。値自体は保持だけして未使用のため
    /// dead_code を明示的に許可する。
    #[serde(default)]
    #[allow(dead_code)]
    pub stay: Option<String>,
}

#[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
fn rgba_u8(c: [f64; 4]) -> [u8; 4] {
    c.map(to_u8)
}

/// V8 の `op_text_style` が受ける `TextStyle`。`noiria_core::text::TextStyle` を直接 serde で受けると
/// 整数フィールドが `f64 as T`（切り捨て）になり tsint（四捨五入）と食い違うので、数値は f64 で受けて
/// `numconv` の規則で落とす。省略したフィールドは既定値のまま。
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
#[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
pub struct TextStyleArgs {
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub line_height: Option<f32>,
    pub color: Option<[f64; 4]>,
    pub outline_color: Option<[f64; 4]>,
    #[serde(deserialize_with = "de_opt_u32")]
    pub outline_width: Option<u32>,
    #[serde(deserialize_with = "de_opt_u32")]
    pub box_w: Option<u32>,
    #[serde(deserialize_with = "de_opt_u32")]
    pub box_h: Option<u32>,
    #[serde(deserialize_with = "de_opt_i32")]
    pub margin_x: Option<i32>,
    #[serde(deserialize_with = "de_opt_i32")]
    pub margin_y: Option<i32>,
    pub align: Option<TextAlign>,
}

#[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
impl TextStyleArgs {
    pub fn into_style(self) -> TextStyle {
        let mut s = TextStyle::default();
        if let Some(v) = self.font_family {
            s.font_family = v;
        }
        if let Some(v) = self.font_size {
            s.font_size = v;
        }
        if let Some(v) = self.line_height {
            s.line_height = v;
        }
        if let Some(v) = self.color {
            s.color = rgba_u8(v);
        }
        if let Some(v) = self.outline_color {
            s.outline_color = rgba_u8(v);
        }
        if let Some(v) = self.outline_width {
            s.outline_width = v;
        }
        if let Some(v) = self.box_w {
            s.box_w = v;
        }
        if let Some(v) = self.box_h {
            s.box_h = v;
        }
        if let Some(v) = self.margin_x {
            s.margin_x = v;
        }
        if let Some(v) = self.margin_y {
            s.margin_y = v;
        }
        if let Some(v) = self.align {
            s.align = v;
        }
        s
    }
}

/// V8 の `op_text_print` が受ける 1 セグメント（`TextStyleArgs` と同じ理由で色だけ f64 で受ける）。
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
#[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
pub struct TextSegmentArgs {
    pub text: String,
    pub color: Option<[f64; 4]>,
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub edge_color: Option<[f64; 4]>,
    pub ruby: Option<String>,
    pub locate: Option<LocateArgs>,
    pub indent: Option<bool>,
    pub inline_image: Option<String>,
    pub font_family: Option<String>,
}

#[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
impl TextSegmentArgs {
    pub fn into_segment(self) -> TextSegment {
        TextSegment {
            text: self.text,
            color: self.color.map(rgba_u8),
            size: self.size,
            bold: self.bold,
            edge_color: self.edge_color.map(rgba_u8),
            ruby: self.ruby,
            locate: self.locate,
            indent: self.indent,
            inline_image: self.inline_image,
            font_family: self.font_family,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PrintResult {
    pub total: usize,
    pub added: usize,
}

/// テキストレイアウトエンジンとレイヤごとの蓄積テキスト（旧 `state::Texts`）。
pub struct Texts {
    pub engine: TextEngine,
    pub styles: HashMap<u32, TextStyle>,
    pub segments: HashMap<u32, Vec<TextSegment>>,
    /// 直近レイアウトの総グリフ数（追記時の再レイアウトを避けるキャッシュ）
    pub glyph_counts: HashMap<u32, usize>,
}

impl Texts {
    pub fn new() -> Self {
        Self {
            engine: TextEngine::new(),
            styles: HashMap::new(),
            segments: HashMap::new(),
            glyph_counts: HashMap::new(),
        }
    }
}

impl Default for Texts {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
struct PlaybackControl {
    pending_seek: Option<usize>,
    period_frame: Option<usize>,
    segment_loop: Option<(usize, usize)>,
    segment_revision: Arc<AtomicU64>,
    #[cfg(test)]
    before_commit_hook: Option<VideoCommitHook>,
}

#[cfg(test)]
struct VideoCommitHook {
    reached: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
impl VideoCommitHook {
    fn wait(self) {
        if self.reached.send(()).is_ok()
            && self.release.recv_timeout(Duration::from_secs(2)).is_err()
        {
            log::error!("video commit test hook timed out");
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VideoEventKind {
    Loop,
    Period,
    Prepare,
    SegLoop,
}

impl VideoEventKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "loop" => Some(Self::Loop),
            "period" => Some(Self::Period),
            "prepare" => Some(Self::Prepare),
            "segLoop" => Some(Self::SegLoop),
            _ => None,
        }
    }
}

#[derive(Default)]
struct VideoEvents {
    loop_count: u64,
    period_count: u64,
    prepare_count: u64,
    seg_loop_count: u64,
    displayed_count: u64,
    last_displayed_frame: Option<usize>,
}

impl VideoEvents {
    fn signal(&mut self, kind: VideoEventKind) {
        let count = match kind {
            VideoEventKind::Loop => &mut self.loop_count,
            VideoEventKind::Period => &mut self.period_count,
            VideoEventKind::Prepare => &mut self.prepare_count,
            VideoEventKind::SegLoop => &mut self.seg_loop_count,
        };
        *count = count.saturating_add(1);
    }

    fn take(&mut self, kind: VideoEventKind) -> bool {
        let count = match kind {
            VideoEventKind::Loop => &mut self.loop_count,
            VideoEventKind::Period => &mut self.period_count,
            VideoEventKind::Prepare => &mut self.prepare_count,
            VideoEventKind::SegLoop => &mut self.seg_loop_count,
        };
        if *count == 0 {
            false
        } else {
            *count -= 1;
            true
        }
    }

    fn take_any(&mut self) -> bool {
        [
            VideoEventKind::Period,
            VideoEventKind::Loop,
            VideoEventKind::Prepare,
            VideoEventKind::SegLoop,
        ]
        .into_iter()
        .any(|kind| self.take(kind))
    }

    fn clear(&mut self, kind: VideoEventKind) {
        match kind {
            VideoEventKind::Loop => self.loop_count = 0,
            VideoEventKind::Period => self.period_count = 0,
            VideoEventKind::Prepare => self.prepare_count = 0,
            VideoEventKind::SegLoop => self.seg_loop_count = 0,
        }
    }
}

/// 再生中の動画を止めるためのハンドル（旧 `ops_video::VideoHandle`）。
pub struct VideoHandle {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    playing: Arc<AtomicBool>,
    control: Arc<Mutex<PlaybackControl>>,
    events: Arc<Mutex<VideoEvents>>,
    fps: f64,
}

/// [openvideo]/[preparevideo]（W6）: 再生スレッドをまだ起動していない、開いただけの動画。
/// `playvideo` がこれを取り出して再生スレッドを起動する。
pub struct PreparedVideo {
    pub id: u32,
    pub source: Box<dyn VideoSource>,
    pub looping: bool,
    initial_index: usize,
    control: Arc<Mutex<PlaybackControl>>,
    events: Arc<Mutex<VideoEvents>>,
}

/// 動画再生状態。同時再生は 1 本のみを想定（旧 `ops_video::Videos`）。
pub struct Videos {
    pub current: Option<VideoHandle>,
    /// [openvideo]/[preparevideo] で開いたが、まだ [playvideo] していない動画（W6）。
    pub prepared: Option<PreparedVideo>,
    generation: Arc<AtomicU64>,
}

impl Videos {
    pub fn new() -> Self {
        Self {
            current: None,
            prepared: None,
            generation: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl Default for Videos {
    fn default() -> Self {
        Self::new()
    }
}

struct PlaybackStateGuard(Arc<AtomicBool>);

impl Drop for PlaybackStateGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

const MAX_RECYCLE_PIXMAPS: usize = 4;

fn push_recycle(recycle: &mut Vec<Arc<Pixmap>>, pixmap: Arc<Pixmap>) {
    if recycle.len() >= MAX_RECYCLE_PIXMAPS {
        recycle.remove(0);
    }
    recycle.push(pixmap);
}

fn next_video_pixmap(
    source: &mut dyn VideoSource,
    recycle: &mut Vec<Arc<Pixmap>>,
) -> anyhow::Result<Option<Arc<Pixmap>>> {
    // Arc::get_mut は Scene や ComposeBufs のキャッシュが参照を手放した後にだけ Some を返すので、
    // Arc::ptr_eq ベースの差分検出と衝突せずバッファを再利用できる。
    let mut reusable = recycle
        .iter_mut()
        .position(|p| Arc::get_mut(p).is_some())
        .map(|i| recycle.swap_remove(i));
    if let Some(ref mut arc) = reusable {
        if let Some(pm) = Arc::get_mut(arc) {
            if !source.next_frame_into(&mut pm.data)? {
                recycle.push(reusable.unwrap());
                return Ok(None);
            }
            let (w, h) = (source.width(), source.height());
            assert_eq!(pm.data.len(), (w as usize) * (h as usize) * 4);
            debug_assert!(pm.data.as_chunks::<4>().0.iter().all(|px| px[3] == 255));
            pm.width = w;
            pm.height = h;
            return Ok(reusable);
        }
    }
    let mut rgba = Vec::new();
    if !source.next_frame_into(&mut rgba)? {
        return Ok(None);
    }
    Ok(Some(Arc::new(Pixmap::from_rgba_opaque(
        source.width(),
        source.height(),
        rgba,
    ))))
}

fn video_frame_at_position(fps: f64, position_ms: f64) -> Result<usize, EngineError> {
    if !position_ms.is_finite() || position_ms < 0.0 {
        return Err(format!("動画位置が不正です: {position_ms}ms"));
    }
    let fps = if fps.is_finite() && fps > 0.0 {
        fps
    } else {
        30.0
    };
    let target = (position_ms * fps / 1000.0).floor();
    if !target.is_finite() || target >= usize::MAX as f64 {
        return Err(format!("動画位置が大きすぎます: {position_ms}ms"));
    }
    Ok(target as usize)
}

pub(crate) fn video_optional_frame(frame: f64) -> Result<Option<usize>, EngineError> {
    if frame == -1.0 {
        return Ok(None);
    }
    if !frame.is_finite() || frame < 0.0 || frame > u32::MAX as f64 || frame.floor() != frame {
        return Err(format!("動画フレーム番号が不正です: {frame}"));
    }
    Ok(Some(frame as usize))
}

/// 動画再生スレッド本体。fps 間隔で `next_frame()` を呼び、対象レイヤ
/// (fore/back 両方) の pixmap を差し替えて再描画を要求する。
#[allow(clippy::too_many_arguments)]
fn playback_loop(
    scene: SharedScene,
    wake: WakeSink,
    id: u32,
    mut source: Box<dyn VideoSource>,
    looping: bool,
    initial_index: usize,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    control: Arc<Mutex<PlaybackControl>>,
    events: Arc<Mutex<VideoEvents>>,
    generation: Arc<AtomicU64>,
    playback_generation: u64,
    playing: Arc<AtomicBool>,
) {
    let _playing_guard = PlaybackStateGuard(playing.clone());
    let fps = if source.fps() > 0.0 {
        source.fps() as f64
    } else {
        30.0
    };
    let frame_dur = Duration::from_secs_f64(1.0 / fps);

    let mut next_due = Instant::now();
    let mut index = initial_index;
    let mut pending_pixmap: Option<Arc<Pixmap>> = None;
    let mut recycle: Vec<Arc<Pixmap>> = Vec::new();
    let segment_revision = {
        let control = control.lock_unpoisoned();
        control.segment_revision.clone()
    };
    let mut pending_boundary_event: Option<(VideoEventKind, u64)> = None;
    'playback: while !stop.load(Ordering::Relaxed) {
        if generation.load(Ordering::SeqCst) != playback_generation {
            break;
        }
        if paused.load(Ordering::Relaxed) {
            next_due = Instant::now();
            std::thread::sleep(Duration::from_millis(30));
            continue;
        }

        let requested_seek = control.lock_unpoisoned().pending_seek.take();
        if let Some(target) = requested_seek {
            if let Some(pm) = pending_pixmap.take() {
                push_recycle(&mut recycle, pm);
            }
            pending_boundary_event = None;
            if let Err(e) = source.seek_frame(target) {
                log::error!("動画フレーム {target} へのシークに失敗: {e:#}");
                break 'playback;
            }
            index = target;
            next_due = Instant::now();
        } else if pending_pixmap.is_none() {
            let segment_start = {
                let control = control.lock_unpoisoned();
                control
                    .segment_loop
                    .filter(|&(_, end)| index >= end)
                    .map(|(start, _)| (start, control.segment_revision.load(Ordering::SeqCst)))
            };
            if let Some((start, revision)) = segment_start {
                if let Err(e) = source.seek_frame(start) {
                    log::error!("動画区間の先頭 {start} へのシークに失敗: {e:#}");
                    break 'playback;
                }
                index = start;
                pending_boundary_event = Some((VideoEventKind::SegLoop, revision));
            }
        }

        if paused.load(Ordering::Relaxed) {
            continue;
        }

        if pending_pixmap.is_none() {
            pending_pixmap = match next_video_pixmap(source.as_mut(), &mut recycle) {
                Ok(Some(pixmap)) => Some(pixmap),
                Ok(None) => {
                    let retry = {
                        let _scene = scene.lock_unpoisoned();
                        let retry = if stop.load(Ordering::Relaxed)
                            || generation.load(Ordering::SeqCst) != playback_generation
                        {
                            false
                        } else if paused.load(Ordering::Relaxed) {
                            true
                        } else {
                            let control = control.lock_unpoisoned();
                            control.pending_seek.is_some()
                                || control.segment_loop.is_some_and(|(_, end)| index >= end)
                        };
                        if !retry && !looping {
                            playing.store(false, Ordering::Relaxed);
                        }
                        retry
                    };
                    if retry {
                        continue 'playback;
                    }
                    if stop.load(Ordering::Relaxed)
                        || generation.load(Ordering::SeqCst) != playback_generation
                    {
                        break 'playback;
                    }
                    if looping {
                        if let Err(e) = source.seek_frame(0) {
                            log::error!("動画の巻き戻しに失敗: {e:#}");
                            break 'playback;
                        }
                        index = 0;
                        pending_boundary_event = Some((VideoEventKind::Loop, 0));
                        continue 'playback;
                    }
                    break 'playback;
                }
                Err(e) => {
                    log::error!("動画フレーム {index} のデコードに失敗: {e:#}");
                    break 'playback;
                }
            };
        }

        #[cfg(test)]
        let before_commit_hook = control.lock_unpoisoned().before_commit_hook.take();
        #[cfg(test)]
        if let Some(hook) = before_commit_hook {
            hook.wait();
        }

        let mut discard_pending = false;
        let committed = {
            let mut sc = scene.lock_unpoisoned();
            if stop.load(Ordering::Relaxed)
                || paused.load(Ordering::Relaxed)
                || generation.load(Ordering::SeqCst) != playback_generation
            {
                false
            } else {
                let period_due = {
                    let mut control = control.lock_unpoisoned();
                    if control.pending_seek.is_some()
                        || control.segment_loop.is_some_and(|(_, end)| index >= end)
                    {
                        discard_pending = true;
                        false
                    } else if control.period_frame == Some(index) {
                        control.period_frame = None;
                        true
                    } else {
                        false
                    }
                };
                if discard_pending {
                    false
                } else {
                    let pixmap = pending_pixmap.as_ref().unwrap();
                    if let Some(layer) = sc.layer_mut(Page::Fore, id) {
                        layer.pixmap = Some(pixmap.clone());
                    }
                    if let Some(layer) = sc.layer_mut(Page::Back, id) {
                        layer.pixmap = Some(pixmap.clone());
                    }
                    sc.bump();

                    let mut events = events.lock_unpoisoned();
                    events.displayed_count = events.displayed_count.saturating_add(1);
                    events.last_displayed_frame = Some(index);
                    if let Some((kind, revision)) = pending_boundary_event {
                        if kind != VideoEventKind::SegLoop
                            || segment_revision.load(Ordering::SeqCst) == revision
                        {
                            events.signal(kind);
                        }
                    }
                    if period_due {
                        events.signal(VideoEventKind::Period);
                    }
                    true
                }
            }
        };

        if discard_pending {
            if let Some(pm) = pending_pixmap.take() {
                push_recycle(&mut recycle, pm);
            }
            pending_boundary_event = None;
            continue;
        }
        if !committed {
            if stop.load(Ordering::Relaxed)
                || generation.load(Ordering::SeqCst) != playback_generation
            {
                break;
            }
            continue;
        }

        if let Some(pm) = pending_pixmap.take() {
            push_recycle(&mut recycle, pm);
        }
        pending_boundary_event = None;
        wake.send(UserEvent::Redraw);

        index += 1;
        next_due += frame_dur;
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let now = Instant::now();
        if next_due > now {
            std::thread::sleep(next_due - now);
        } else {
            next_due = now;
        }
    }
}

fn parse_page(page: &str) -> Result<Page, EngineError> {
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
pub struct EngineState {
    pub scene: SharedScene,
    pub wake: WakeSink,
    /// V8 版 `op_trans_wait`（`ops_trans.rs`）専用。tsint 版は自前のポーリングループ
    /// （`tsint_engine::PendingOps::trans`）で待つためこのフィールドを読まない
    /// （engine-v8 を無効化したビルドでは未使用になるので dead_code を明示的に許可する）。
    #[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
    pub trans_notify: Arc<Notify>,
    pub audio_tx: Sender<AudioCmd>,
    pub audio_shared: Arc<AudioShared>,
    pub game_dir: GameDir,
    pub config: GameConfig,
    pub start: Instant,
    /// 現在時刻の供給源。アニメーション（tween/transition/quake/reveal）の
    /// 開始時刻をここから記録することで、ヘッドレス実行時は headless.rs の
    /// フレームループが読む値と一致させる（`crate::clock` 参照）。
    pub clock: Clock,
    /// ヘッドレス専用のロックステップ同期カウンタ（`crate::bridge::InputSync` 参照）。
    /// `op_input_next_event`（V8/tsint 両方）が呼ばれた時点で ack する。windowed 経路では
    /// 誰も観測しないので挙動に影響しない。
    pub input_sync: InputSync,
    /// ヘッドレスの tick ドレインバリア用カウンタ（`crate::bridge::TickSync` 参照）。
    /// tsint は `drive()`、V8 は `op_wait` と外側の event-loop drain から完了を報告する。
    /// windowed 経路では誰も完了世代を待たないので挙動に影響しない。
    pub tick_sync: TickSync,
    /// `op_wait`（V8 版）の仮想 in-flight 数（`crate::bridge::PendingWaits` 参照）。
    /// headless.rs がこれを読んで V8 の tick ドレインバリアの要否を判定する。
    /// tsint/windowed 経路では誰も書かない（engine-v8 未使用ビルドでは未読になるため
    /// dead_code を明示的に許可する）。
    #[cfg_attr(not(feature = "engine-v8"), allow(dead_code))]
    pub pending_waits: PendingWaits,
    pub texts: Texts,
    pub videos: Videos,
    /// タグ振る舞いテストハーネス専用（既定 `None`）。`Some` の場合のみ `log_op` が
    /// 呼び出し列を蓄積する。
    pub op_log: Option<SharedOpLog>,
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scene: SharedScene,
        wake: WakeSink,
        trans_notify: Arc<Notify>,
        audio_tx: Sender<AudioCmd>,
        audio_shared: Arc<AudioShared>,
        game_dir: GameDir,
        config: GameConfig,
        start: Instant,
        clock: Clock,
        input_sync: InputSync,
        tick_sync: TickSync,
        pending_waits: PendingWaits,
        op_log: Option<SharedOpLog>,
    ) -> Self {
        let mut texts = Texts::new();
        texts
            .engine
            .load_game_fonts(load_game_font_bytes(&game_dir));
        let ui_cache = Arc::new(Mutex::new(UiCaches::new()));
        let prefetcher = Prefetcher::new(ui_cache.clone(), game_dir.clone(), config.id.clone());
        Self {
            scene,
            wake,
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
            texts,
            videos: Videos::new(),
            op_log,
            pending_thumb: Mutex::new(None),
            ui_cache,
            prefetcher,
        }
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

    pub fn layer_new(&self, kind: &str) -> Result<u32, EngineError> {
        let kind_enum = match kind {
            "image" => LayerKind::Image,
            "text" => LayerKind::Text,
            _ => return Err(format!("不明なレイヤ種別: {kind}")),
        };
        let id = self.scene.lock_unpoisoned().alloc_layer(kind_enum);
        self.log_op(format!("layer_new {kind} -> {id}"));
        Ok(id)
    }

    pub fn layer_free(&self, id: u32) {
        self.log_op(format!("layer_free {id}"));
        self.with_scene(|scene| scene.free_layer(id));
    }

    pub fn layer_load_image(
        &self,
        id: u32,
        page: &str,
        path: String,
    ) -> Result<(u32, u32), EngineError> {
        self.log_op(format!("layer_load_image {id} {page} {path}"));
        let pixmap = load_image_checked(&self.game_dir, &path)?;
        let size = (pixmap.width, pixmap.height);
        let pixmap = Arc::new(pixmap);
        self.with_layer(page, id, |layer| {
            layer.pixmap = Some(pixmap);
            layer.image_path = Some(path);
        })?;
        Ok(size)
    }

    /// [image]（W5 拡張）: `layer_load_image` の拡張版。clip= で部分矩形を抽出し、
    /// flipud=/fliplr=/grayscale= をロード後に適用してからレイヤへ設定する
    /// （`layer_blit_image` と異なり既存内容を消して置き換える）。
    pub fn layer_load_image_ex(
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
        let mut pixmap = load_image_checked(&self.game_dir, &path)?;
        if let Some(c) = opts.clip {
            checked_dims(c.w, c.h)?;
            pixmap = pixmap.extract_region(c.x, c.y, c.w, c.h);
        }
        if opts.flip_h {
            pixmap = pixmap.flip_h();
        }
        if opts.flip_v {
            pixmap = pixmap.flip_v();
        }
        if opts.grayscale {
            pixmap = pixmap.grayscale();
        }
        let size = (pixmap.width, pixmap.height);
        let pixmap = Arc::new(pixmap);
        self.with_layer(page, id, |layer| {
            layer.pixmap = Some(pixmap);
            layer.image_path = Some(path);
        })?;
        Ok(size)
    }

    pub fn layer_clear_image(&self, id: u32, page: &str) -> Result<(), EngineError> {
        self.log_op(format!("layer_clear_image {id} {page}"));
        self.with_layer(page, id, |layer| {
            layer.pixmap = None;
            layer.image_path = None;
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn layer_fill(
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
    pub fn layer_msgbox(
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
    pub fn layer_panel(&self, id: u32, page: &str, spec_raw: &[f64]) -> Result<(), EngineError> {
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
    pub fn layer_load_image_scaled(
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

    pub fn layer_set_props(
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
    pub fn layer_move(&self, id: u32, page: &str, args: MoveArgs) -> Result<(), EngineError> {
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
    pub fn layer_set_crop(&self, id: u32, page: &str, w: u32) -> Result<(), EngineError> {
        self.log_op(format!("layer_set_crop {id} {page} {w}"));
        self.with_layer(page, id, |layer| {
            layer.crop_w = Some(w);
            layer.crop_tween = None;
        })
    }

    /// 画像の「見える幅」（左端から何画素ぶん合成するか）を、今の値から `to` へ tween する
    /// （アンダーラインが左から伸びる等のモダン UI 用。位置/不透明度の `layer_move` とは独立に動く）。
    pub fn layer_crop_move(
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

    pub fn layer_move_cancel(&self, id: u32, page: &str) -> Result<(), EngineError> {
        self.log_op(format!("layer_move_cancel {id} {page}"));
        self.with_layer(page, id, |layer| {
            layer.tween = None;
            layer.tween_queue.clear();
        })
    }

    /// 全レイヤの tween を即座に完了させる（スキップモード用）。[stopmove]（W5）もこれを
    /// そのまま流用する（全レイヤの move を目標値で即完了させるという効果が一致するため）。
    pub fn layer_move_skip(&self) {
        self.log_op("layer_move_skip");
        self.with_scene(|scene| scene.skip_all_tweens());
    }

    /// [move path=]（W5/B12）: 複数区間の move。`points` は区間ごとの `MoveArgs`
    /// （各区間の toX/toY/toOpacity/timeMs/accel）。区間の toX/toY/toOpacity を省略した場合は
    /// 直前の区間の値（先頭区間は現在の実効値）を引き継ぐ。第一区間は現在値からの通常の
    /// `layer_move` と同じ開始をし、残りは `tween_queue` へ積んで `Scene::settle_finished_tweens`
    /// が順につなげる。`spline=true` では現在位置を先頭点とする自然三次スプラインを使う。
    pub fn layer_move_path(
        &self,
        id: u32,
        page: &str,
        points: Vec<MoveArgs>,
        spline: bool,
    ) -> Result<(), EngineError> {
        self.layer_move_path_inner(id, page, points, spline, None)
    }

    pub fn layer_move_path_by_distance(
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
    pub fn layer_moving(&self) -> bool {
        let now = self.clock.now();
        let scene = self.scene.lock_unpoisoned();
        scene.fore.values().any(|l| {
            l.tween.as_ref().is_some_and(|t| !t.is_finished(now)) || !l.tween_queue.is_empty()
        })
    }

    /// KAG の backlay。id < 0 で全レイヤ。
    pub fn backlay(&self, id: i32) {
        self.log_op(format!("backlay {id}"));
        self.with_scene(|scene| scene.backlay(if id < 0 { None } else { Some(id as u32) }));
    }

    /// KAG の [copylay]（W3、W5 で `copy_position` を追加）: 異なるレイヤ間で内容をコピーする。
    /// `copy_position=false` はコピー先の x/y を変えない（メッセージレイヤ宛の既知バグ修正、
    /// `Scene::copy_layer` のコメント参照）。
    pub fn layer_copy(
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
    pub fn layer_blit_image(
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
        let src = load_image_checked(&self.game_dir, path)?;
        let region = match clip {
            Some(c) => {
                checked_dims(c.w, c.h)?;
                src.extract_region(c.x, c.y, c.w, c.h)
            }
            None => src,
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
    pub fn quake(&self, ax: i32, ay: i32, ms: u32) {
        self.log_op(format!("quake ax={ax} ay={ay} ms={ms}"));
        let now = self.clock.now();
        self.with_scene(|scene| {
            scene.quake = Some((ax, ay, now, ms));
        });
    }

    /// [stopquake]（W5）: 画面揺れを即座に停止する。
    pub fn quake_stop(&self) {
        self.log_op("quake_stop");
        self.with_scene(|scene| {
            scene.quake = None;
        });
    }

    // ---- テキスト ----

    pub fn text_style(&mut self, id: u32, style: TextStyle) {
        self.log_op(format!(
            "text_style {id} align={:?} line_height={} font_family={}",
            style.align, style.line_height, style.font_family
        ));
        self.texts.styles.insert(id, style);
    }

    pub fn text_print(
        &mut self,
        id: u32,
        segments: Vec<TextSegment>,
        append: bool,
        cps: f64,
    ) -> Result<PrintResult, EngineError> {
        self.log_op(format!(
            "text_print {id} append={append} cps={cps} segs={segments:?}"
        ));
        // [graph]（インライン画像, W3）: text.rs は assets への依存を増やさない設計にしたため
        // （`docs/implementation-notes.md` 2026-07-11 W3 参照）、実ファイル I/O はここで行い、
        // ロード済みの Pixmap を TextEngine の画像キャッシュへ渡す。
        for seg in &segments {
            if let Some(path) = &seg.inline_image {
                if !self.texts.engine.has_image(path) {
                    match load_image_checked(&self.game_dir, path) {
                        Ok(pixmap) => self.texts.engine.cache_image(path.clone(), pixmap),
                        Err(e) => log::warn!("[graph] 画像の読み込みに失敗しました: {path}: {e:#}"),
                    }
                }
            }
        }

        let (raster, from_glyph) = {
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
            let entry = texts.segments.entry(id).or_default();
            entry.extend(segments);
            let all = entry.clone();
            let raster = texts.engine.raster_segments(&all, &style);
            texts.glyph_counts.insert(id, raster.glyphs.len());
            (raster, from_glyph)
        };

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

    pub fn text_reflow(&mut self, id: u32) -> Result<(), EngineError> {
        self.log_op(format!("text_reflow {id}"));
        let raster = {
            let texts = &mut self.texts;
            let style = texts.styles.entry(id).or_default().clone();
            let segments = texts.segments.entry(id).or_default().clone();
            let raster = texts.engine.raster_segments(&segments, &style);
            texts.glyph_counts.insert(id, raster.glyphs.len());
            Arc::new(raster)
        };
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
    pub fn text_complete(&self, id: u32) {
        self.log_op(format!("text_complete {id}"));
        let _ = self.with_layer("fore", id, |layer| {
            if let Some(text) = &mut layer.text {
                text.reveal.start = None;
            }
        });
    }

    /// リベールが完了しているか。
    pub fn text_is_done(&self, id: u32) -> bool {
        let scene = self.scene.lock_unpoisoned();
        scene
            .layer(Page::Fore, id)
            .and_then(|l| l.text.as_ref())
            .map(|t| t.reveal.is_done(t.raster.glyphs.len(), self.clock.now()))
            .unwrap_or(true)
    }

    pub fn text_clear(&mut self, id: u32) {
        self.log_op(format!("text_clear {id}"));
        self.texts.segments.remove(&id);
        self.texts.glyph_counts.remove(&id);
        let _ = self.with_layer("fore", id, |layer| {
            layer.text = None;
        });
    }

    /// グリフのヒット矩形（リンク選択肢・メニュー行判定用）。低頻度（実クリック時のみ）
    /// なので op_log にも記録する（タグ振る舞いテストハーネスがリンクの実座標を知る手段）。
    pub fn text_rects(&self, id: u32) -> Vec<GlyphRect> {
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

    pub fn trans_begin(&self, args: TransArgs) -> Result<(), EngineError> {
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
                let (w, h, rgba) = self
                    .game_dir
                    .load_image_rgba(rule_path)
                    .map_err(|e| e.to_string())?;
                checked_dims(w, h).map_err(|e| format!("{rule_path}: {e}"))?;
                let plane = rule_plane_from_rgba(w, h, &rgba, scene_w, scene_h);
                TransMethod::Universal {
                    rule: Arc::new(plane),
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

    pub fn trans_skip(&self) {
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
    pub fn trans_active(&self) -> bool {
        self.scene.lock_unpoisoned().transition.is_some()
    }

    // ---- オーディオ ----

    #[allow(clippy::too_many_arguments)]
    pub fn audio_play(
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

    pub fn audio_stop(&self, ch: &str, fade_out_ms: u32) -> Result<(), EngineError> {
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

    pub fn audio_volume(&self, ch: &str, volume: f64) -> Result<(), EngineError> {
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
    pub fn audio_busy(&self, ch: usize) -> bool {
        self.audio_shared.is_playing(ch) || self.audio_shared.is_play_pending(ch)
    }

    pub fn audio_is_playing(&self, ch: &str) -> Result<bool, EngineError> {
        let ch = ch_of(ch)?;
        Ok(self.audio_shared.is_playing(ch))
    }

    pub fn audio_position(&self, ch: &str) -> Result<u64, EngineError> {
        let ch = ch_of(ch)?;
        Ok(self.audio_shared.position_ms(ch))
    }

    pub fn audio_pause(&self, ch: &str) -> Result<(), EngineError> {
        self.log_op(format!("audio_pause {ch}"));
        let ch = ch_of(ch)?;
        let _ = self.audio_tx.send(AudioCmd::Pause { ch });
        Ok(())
    }

    pub fn audio_resume(&self, ch: &str) -> Result<(), EngineError> {
        self.log_op(format!("audio_resume {ch}"));
        let ch = ch_of(ch)?;
        let _ = self.audio_tx.send(AudioCmd::Resume { ch });
        Ok(())
    }

    /// `then`: "none" | "stop" | "pause"（フェード完了時の遷移）。
    pub fn audio_fade_to(
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

    pub fn audio_is_fading(&self, ch: &str) -> Result<bool, EngineError> {
        Ok(self.audio_shared.is_fading(ch_of(ch)?))
    }

    pub fn audio_is_finished(&self, ch: &str) -> Result<bool, EngineError> {
        Ok(self.audio_shared.is_finished(ch_of(ch)?))
    }

    // ---- システム ----

    /// アプリ起動からのミリ秒。
    pub fn now(&self) -> f64 {
        self.clock.now().duration_since(self.start).as_secs_f64() * 1000.0
    }

    /// `[eval]`/W2〜W6 の各タグが `api.sys.log(...)` 経由で出す実行証跡ログ。多くの
    /// W2/W4/W6 タグ（bgmopt/pausebgm/button/timeout/tempsave/save/... 等）はレイヤ/オーディオ
    /// ops を一切呼ばずこのログだけが観測可能な効果なので、タグ振る舞いテストハーネスのために
    /// `op_log` にも記録する（本番既定 `None` の間は従来通り `log()` を呼ぶだけ）。
    pub fn sys_log(&self, level: &str, msg: &str) {
        self.log_op(format!("log {level} {msg}"));
        log(level, msg);
    }

    pub fn quit(&self) {
        self.log_op("quit");
        self.wake.send(UserEvent::Quit);
    }

    pub fn set_title(&self, title: String) {
        self.log_op(format!("set_title {title}"));
        self.wake.send(UserEvent::SetTitle(title));
    }

    /// ゲームディレクトリ内のテキストファイル (.ks 等) を読む。
    pub fn read_text(&self, rel: &str) -> Result<String, EngineError> {
        self.game_dir.read_text(rel).map_err(|e| e.to_string())
    }

    pub fn file_exists(&self, rel: &str) -> bool {
        self.game_dir.exists(rel)
    }

    pub fn save_write(&self, slot: &str, json: &str) -> Result<(), EngineError> {
        self.log_op(format!("save_write {slot}"));
        savedata::write_slot(&self.config.id, slot, json).map_err(|e| e.to_string())
    }

    pub fn save_read(&self, slot: &str) -> Result<Option<String>, EngineError> {
        self.log_op(format!("save_read {slot}"));
        savedata::read_slot(&self.config.id, slot).map_err(|e| e.to_string())
    }

    pub fn save_list(&self) -> Result<Vec<SlotInfo>, EngineError> {
        self.log_op("save_list");
        savedata::list_slots(&self.config.id).map_err(|e| e.to_string())
    }

    /// [erasebookmark] の真の削除（W7）。存在しないスロットは no-op。
    pub fn save_delete(&self, slot: &str) -> Result<(), EngineError> {
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
    pub fn screenshot(&self, slot: &str) -> Result<(), EngineError> {
        self.log_op(format!("screenshot {slot}"));
        let path = savedata::thumb_path(&self.config.id, slot).map_err(|e| e.to_string())?;
        let out = self.compose_current()?;
        save_png(&out, &path).map_err(|e| e.to_string())
    }

    /// 現在のシーンを合成し、セーブ一覧用の規定寸法 (`THUMB_W`x`THUMB_H`) へ縮小して保持する。
    /// ファイルへは書かない（`thumb_save` で書く）。
    pub fn thumb_capture(&self) {
        let _ = self.thumb_capture_sized(0, 0);
    }

    /// `thumb_capture` の寸法指定版。0 のどちらかを含むなら規定寸法（モダン UI は大きいカードに合わせて
    /// 規定より大きく撮る。一覧は読み込み時に表示寸法へ縮める）。
    pub fn thumb_capture_sized(&self, w: u32, h: u32) -> Result<(), EngineError> {
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
    pub fn thumb_save(&self, slot: &str) -> Result<(), EngineError> {
        self.log_op(format!("thumb_save {slot}"));
        if self.pending_thumb.lock_unpoisoned().is_none() {
            self.thumb_capture();
        }
        let guard = self.pending_thumb.lock_unpoisoned();
        let thumb = guard.as_ref().expect("直前に取得済み");
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
    pub fn layer_load_save_thumb_sized(
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
    pub fn prefetch_save_thumbs(&self, w: u32, h: u32) {
        self.log_op(format!("prefetch_save_thumbs {w}x{h}"));
        self.prefetcher.thumbs(w, h);
    }

    /// ギャラリーのサムネイル（ゲーム資産の画像を w x h へ縮小したもの）を別スレッドで先に作っておく。
    pub fn prefetch_images_scaled(&self, paths: Vec<String>, w: u32, h: u32) {
        self.log_op(format!("prefetch_images_scaled {} {w}x{h}", paths.len()));
        self.prefetcher.scaled(paths, w, h);
    }

    /// セーブ一覧に出す保存日時（`YYYY/MM/DD HH:MM`、ローカル時刻）。ヘッドレス（仮想クロック）では
    /// フレームを決定論に保つため固定値を返す。
    pub fn wall_clock_string(&self) -> String {
        if !self.clock.is_real() {
            return "2026/01/01 12:00".to_string();
        }
        noiria_core::localtime::now_local()
            .map(|t| t.format())
            .unwrap_or_default()
    }

    // ---- 動画 ----

    fn invalidate_current_video(&mut self) -> u64 {
        let _scene = self.scene.lock_unpoisoned();
        if let Some(current) = self.videos.current.as_ref() {
            current.stop.store(true, Ordering::Relaxed);
            current.playing.store(false, Ordering::Relaxed);
            *current.events.lock_unpoisoned() = VideoEvents::default();
        }
        self.videos.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// 開いた動画ソースの再生スレッドを起動する（`video_play`/`video_play_prepared` 共用）。
    #[allow(clippy::too_many_arguments)]
    fn spawn_video_playback(
        &mut self,
        id: u32,
        source: Box<dyn VideoSource>,
        looping: bool,
        initial_index: usize,
        control: Arc<Mutex<PlaybackControl>>,
        events: Arc<Mutex<VideoEvents>>,
    ) {
        let playback_generation = self.invalidate_current_video();
        let fps = source.fps() as f64;
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let playing = Arc::new(AtomicBool::new(true));
        self.videos.current = Some(VideoHandle {
            stop: stop.clone(),
            paused: paused.clone(),
            playing: playing.clone(),
            control: control.clone(),
            events: events.clone(),
            fps,
        });

        let scene = self.scene.clone();
        let wake = self.wake.clone();
        let generation = self.videos.generation.clone();
        std::thread::Builder::new()
            .name("noiria-video".into())
            .spawn(move || {
                playback_loop(
                    scene,
                    wake,
                    id,
                    source,
                    looping,
                    initial_index,
                    stop,
                    paused,
                    control,
                    events,
                    generation,
                    playback_generation,
                    playing,
                )
            })
            .expect("動画再生スレッドの起動に失敗");
    }

    fn video_play_from(
        &mut self,
        id: u32,
        path: &str,
        looping: bool,
        frame: Option<usize>,
        position_ms: Option<f64>,
    ) -> Result<(), EngineError> {
        let mut source = self.game_dir.open_video(path).map_err(|e| e.to_string())?;
        {
            let sc = self.scene.lock_unpoisoned();
            if sc.layer(Page::Fore, id).is_none() {
                return Err(format!("レイヤが存在しません: {id}"));
            }
        }

        let target = if let Some(frame) = frame {
            Some(frame)
        } else if let Some(position_ms) = position_ms {
            Some(video_frame_at_position(source.fps() as f64, position_ms)?)
        } else {
            None
        };
        if let Some(target) = target {
            source.seek_frame(target).map_err(|e| e.to_string())?;
        }

        self.spawn_video_playback(
            id,
            source,
            looping,
            target.unwrap_or(0),
            Arc::new(Mutex::new(PlaybackControl::default())),
            Arc::new(Mutex::new(VideoEvents::default())),
        );
        Ok(())
    }

    pub fn video_play(&mut self, id: u32, path: &str, looping: bool) -> Result<(), EngineError> {
        self.log_op(format!("video_play {id} {path} loop={looping}"));
        self.video_play_from(id, path, looping, None, None)
    }

    pub fn video_play_at(
        &mut self,
        id: u32,
        path: &str,
        looping: bool,
        frame: Option<usize>,
        position_ms: Option<f64>,
    ) -> Result<(), EngineError> {
        self.log_op(format!(
            "video_play_at {id} {path} loop={looping} frame={frame:?} position={position_ms:?}"
        ));
        self.video_play_from(id, path, looping, frame, position_ms)
    }

    pub fn video_stop(&mut self) {
        self.log_op("video_stop");
        let _scene = self.scene.lock_unpoisoned();
        if let Some(current) = self.videos.current.as_ref() {
            current.stop.store(true, Ordering::Relaxed);
            current.playing.store(false, Ordering::Relaxed);
            *current.events.lock_unpoisoned() = VideoEvents::default();
        }
        self.videos.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn video_is_playing(&self) -> bool {
        self.videos
            .current
            .as_ref()
            .is_some_and(|c| c.playing.load(Ordering::Relaxed))
    }

    /// [openvideo]（W6）: 動画を開くだけで再生は開始しない。[playvideo] が使う
    /// `prepared` スロットへ格納する（同一レイヤ向けの既存 prepared があれば置き換える）。
    pub fn video_open(&mut self, id: u32, path: &str, looping: bool) -> Result<(), EngineError> {
        self.log_op(format!("video_open {id} {path} loop={looping}"));
        let source = self.game_dir.open_video(path).map_err(|e| e.to_string())?;
        {
            let sc = self.scene.lock_unpoisoned();
            if sc.layer(Page::Fore, id).is_none() {
                return Err(format!("レイヤが存在しません: {id}"));
            }
        }
        self.videos.prepared = Some(PreparedVideo {
            id,
            source,
            looping,
            initial_index: 0,
            control: Arc::new(Mutex::new(PlaybackControl::default())),
            events: Arc::new(Mutex::new(VideoEvents::default())),
        });
        Ok(())
    }

    fn prepare_opened_video(&mut self, id: u32) -> Result<(), EngineError> {
        let (pixmap, events) = {
            let prepared = self
                .videos
                .prepared
                .as_mut()
                .ok_or_else(|| "準備対象として開かれた動画がありません".to_string())?;
            if prepared.id != id {
                return Err(format!(
                    "開かれた動画のレイヤが一致しません（準備済み={}, 指定={id}）",
                    prepared.id
                ));
            }
            let pixmap = next_video_pixmap(prepared.source.as_mut(), &mut Vec::new())
                .map_err(|e| e.to_string())?;
            prepared
                .source
                .seek_frame(prepared.initial_index)
                .map_err(|e| e.to_string())?;
            (pixmap, prepared.events.clone())
        };

        self.invalidate_current_video();
        if let Some(pixmap) = pixmap {
            self.with_layer("fore", id, |layer| layer.pixmap = Some(pixmap.clone()))?;
            self.with_layer("back", id, |layer| layer.pixmap = Some(pixmap))?;
        }
        events.lock_unpoisoned().signal(VideoEventKind::Prepare);
        Ok(())
    }

    /// [preparevideo]（W6）: `video_open` に加えて現在の開始フレームをデコードして表示する。
    /// 表示後は同じフレーム位置へ戻し、再生開始位置を保持する。
    pub fn video_prepare(&mut self, id: u32, path: &str, looping: bool) -> Result<(), EngineError> {
        self.log_op(format!("video_prepare {id} {path} loop={looping}"));
        self.video_open(id, path, looping)?;
        self.prepare_opened_video(id)
    }

    pub fn video_prepare_opened(&mut self, id: u32) -> Result<(), EngineError> {
        self.log_op(format!("video_prepare_opened {id}"));
        self.prepare_opened_video(id)
    }

    pub fn video_seek_frame(&mut self, target: usize) -> Result<(), EngineError> {
        self.log_op(format!("video_seek_frame {target}"));
        if let Some(prepared) = self.videos.prepared.as_mut() {
            prepared
                .source
                .seek_frame(target)
                .map_err(|e| e.to_string())?;
            prepared.initial_index = target;
        }
        let _scene = self.scene.lock_unpoisoned();
        if let Some(current) = self.videos.current.as_ref() {
            current.control.lock_unpoisoned().pending_seek = Some(target);
        }
        Ok(())
    }

    pub fn video_seek_position(&mut self, position_ms: f64) -> Result<(), EngineError> {
        self.log_op(format!("video_seek_position {position_ms}"));
        if !position_ms.is_finite() || position_ms < 0.0 {
            return Err(format!("動画位置が不正です: {position_ms}ms"));
        }
        if let Some(prepared) = self.videos.prepared.as_mut() {
            let target = video_frame_at_position(prepared.source.fps() as f64, position_ms)?;
            prepared
                .source
                .seek_frame(target)
                .map_err(|e| e.to_string())?;
            prepared.initial_index = target;
        }
        let _scene = self.scene.lock_unpoisoned();
        if let Some(current) = self.videos.current.as_ref() {
            let target = video_frame_at_position(current.fps, position_ms)?;
            current.control.lock_unpoisoned().pending_seek = Some(target);
        }
        Ok(())
    }

    pub fn video_set_period_event(&mut self, frame: usize) {
        self.log_op(format!("video_set_period_event {frame}"));
        let _scene = self.scene.lock_unpoisoned();
        if let Some(prepared) = self.videos.prepared.as_ref() {
            let mut control = prepared.control.lock_unpoisoned();
            control.period_frame = Some(frame);
            drop(control);
            prepared
                .events
                .lock_unpoisoned()
                .clear(VideoEventKind::Period);
        }
        if let Some(current) = self.videos.current.as_ref() {
            let mut control = current.control.lock_unpoisoned();
            control.period_frame = Some(frame);
            drop(control);
            current.events.lock_unpoisoned().clear(VideoEventKind::Period);
        }
    }

    pub fn video_cancel_period_event(&mut self) {
        self.log_op("video_cancel_period_event");
        let _scene = self.scene.lock_unpoisoned();
        if let Some(prepared) = self.videos.prepared.as_ref() {
            let mut control = prepared.control.lock_unpoisoned();
            control.period_frame = None;
            drop(control);
            prepared
                .events
                .lock_unpoisoned()
                .clear(VideoEventKind::Period);
        }
        if let Some(current) = self.videos.current.as_ref() {
            let mut control = current.control.lock_unpoisoned();
            control.period_frame = None;
            drop(control);
            current.events.lock_unpoisoned().clear(VideoEventKind::Period);
        }
    }

    pub fn video_set_segment_loop(&mut self, start: usize, end: usize) -> Result<(), EngineError> {
        if start >= end {
            return Err(format!(
                "動画区間は start < end である必要があります: {start}..{end}"
            ));
        }
        self.log_op(format!("video_set_segment_loop {start} {end}"));
        let _scene = self.scene.lock_unpoisoned();
        if let Some(prepared) = self.videos.prepared.as_ref() {
            let mut control = prepared.control.lock_unpoisoned();
            control.segment_revision.fetch_add(1, Ordering::SeqCst);
            control.segment_loop = Some((start, end));
            drop(control);
            prepared
                .events
                .lock_unpoisoned()
                .clear(VideoEventKind::SegLoop);
        }
        if let Some(current) = self.videos.current.as_ref() {
            let mut control = current.control.lock_unpoisoned();
            control.segment_revision.fetch_add(1, Ordering::SeqCst);
            control.segment_loop = Some((start, end));
            drop(control);
            current
                .events
                .lock_unpoisoned()
                .clear(VideoEventKind::SegLoop);
        }
        Ok(())
    }

    pub fn video_cancel_segment_loop(&mut self) {
        self.log_op("video_cancel_segment_loop");
        let _scene = self.scene.lock_unpoisoned();
        if let Some(prepared) = self.videos.prepared.as_ref() {
            let mut control = prepared.control.lock_unpoisoned();
            control.segment_revision.fetch_add(1, Ordering::SeqCst);
            control.segment_loop = None;
            drop(control);
            prepared
                .events
                .lock_unpoisoned()
                .clear(VideoEventKind::SegLoop);
        }
        if let Some(current) = self.videos.current.as_ref() {
            let mut control = current.control.lock_unpoisoned();
            control.segment_revision.fetch_add(1, Ordering::SeqCst);
            control.segment_loop = None;
            drop(control);
            current
                .events
                .lock_unpoisoned()
                .clear(VideoEventKind::SegLoop);
        }
    }

    pub fn video_take_event(&mut self, kind: &str) -> bool {
        let parsed = VideoEventKind::parse(kind);
        if kind == "any" || parsed == Some(VideoEventKind::Prepare) {
            if let Some(prepared) = self.videos.prepared.as_ref() {
                let mut events = prepared.events.lock_unpoisoned();
                let taken = if kind == "any" {
                    events.take_any()
                } else {
                    events.take(VideoEventKind::Prepare)
                };
                if taken {
                    return true;
                }
            }
        }
        let Some(current) = self.videos.current.as_ref() else {
            return false;
        };
        let mut events = current.events.lock_unpoisoned();
        if kind == "any" {
            events.take_any()
        } else {
            parsed.is_some_and(|event| events.take(event))
        }
    }

    pub fn video_generation(&self) -> f64 {
        self.videos.generation.load(Ordering::SeqCst) as f64
    }

    /// [playvideo]（storage= 省略時、W6）: `prepared`（openvideo/preparevideo で開いた動画）の
    /// 再生を開始する。準備済みの動画が無い、または別レイヤ向けのものなら失敗させる。
    pub fn video_play_prepared(&mut self, id: u32) -> Result<(), EngineError> {
        self.log_op(format!("video_play_prepared {id}"));
        let prepared = self
            .videos
            .prepared
            .take()
            .ok_or_else(|| "準備済みの動画がありません".to_string())?;
        if prepared.id != id {
            return Err(format!(
                "準備済みの動画のレイヤが一致しません（準備済み={}, 指定={id}）",
                prepared.id
            ));
        }
        self.spawn_video_playback(
            id,
            prepared.source,
            prepared.looping,
            prepared.initial_index,
            prepared.control,
            prepared.events,
        );
        Ok(())
    }

    /// [pausevideo]（W6）: 再生中の動画をその場で一時停止する（再生位置は保持）。
    pub fn video_pause(&mut self) {
        self.log_op("video_pause");
        let _scene = self.scene.lock_unpoisoned();
        if let Some(current) = self.videos.current.as_ref() {
            current.paused.store(true, Ordering::Relaxed);
        }
    }

    /// [resumevideo]（W6）: 一時停止からの再開。
    pub fn video_resume(&mut self) {
        self.log_op("video_resume");
        let _scene = self.scene.lock_unpoisoned();
        if let Some(current) = self.videos.current.as_ref() {
            current.paused.store(false, Ordering::Relaxed);
        }
    }

    /// [rewindvideo]（W6）: 再生中/一時停止中の動画を先頭へ巻き戻す。
    pub fn video_rewind(&mut self) {
        self.log_op("video_rewind");
        if let Some(prepared) = self.videos.prepared.as_mut() {
            match prepared.source.seek_frame(0) {
                Ok(()) => prepared.initial_index = 0,
                Err(e) => log::error!("準備済み動画の巻き戻しに失敗: {e:#}"),
            }
        }
        let _scene = self.scene.lock_unpoisoned();
        if let Some(current) = self.videos.current.as_ref() {
            current.control.lock_unpoisoned().pending_seek = Some(0);
        }
    }

    /// 一時停止状態か（テスト・デバッグ用に公開）。
    pub fn video_is_paused(&self) -> bool {
        self.videos
            .current
            .as_ref()
            .is_some_and(|c| c.paused.load(Ordering::Relaxed))
    }
}

/// ログ出力。`OpState`/`EngineState` に依存しない自由関数（旧 `ops_sys::op_log`）。
pub fn log(level: &str, msg: &str) {
    match level {
        "error" => log::error!("[script] {msg}"),
        "warn" => log::warn!("[script] {msg}"),
        "debug" => log::debug!("[script] {msg}"),
        _ => log::info!("[script] {msg}"),
    }
}

#[cfg(test)]
mod tests {
    //! `layer_blit_image`（W4）: V8/tsint を介さず `EngineState` を直接叩く単体テスト
    //! （`script::mod::js_tests::move_tween_reaches_to_value_in_v8` と同じ
    //! 「実ファイル + 実 GameDir」方式だが、こちらはスクリプトエンジンを経由しない
    //! ぶん軽量）。3 状態ボタン画像（横に 3 分割、各 4x4px の色違い）を模した
    //! PNG を一時ゲームディレクトリへ書き出し、clip= で中央（緑）だけを
    //! 切り出してレイヤへブリットできることを確認する。

    use super::*;
    use crossbeam_channel::unbounded;
    use noiria_core::assets::{save_png, GameDir};
    use noiria_core::scene::{Page, Scene};
    use std::sync::Mutex;

    fn make_engine_state(dir: &std::path::Path) -> EngineState {
        let (game_dir, config) = GameDir::open(dir).expect("GameDir::open");
        let (user_tx, _user_rx) = unbounded();
        let (audio_tx, audio_rx) = unbounded();
        drop(audio_rx);
        EngineState::new(
            Arc::new(Mutex::new(Scene::new(64, 64))),
            WakeSink::Channel(user_tx),
            Arc::new(Notify::new()),
            audio_tx,
            Arc::new(AudioShared::default()),
            game_dir,
            config,
            Instant::now(),
            Clock::Real,
            InputSync::new(),
            TickSync::new(),
            PendingWaits::new(),
            None,
        )
    }

    #[test]
    fn layer_blit_image_clips_and_composites_onto_existing_layer() {
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("noiria-blit-image-test-{ns}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("game.json"),
            r#"{"title":"t","id":"noiria-blit-image-test","main":"main.ts"}"#,
        )
        .unwrap();

        // 12x4 の画像を 4x4 の 3 マス (赤/緑/青) に分割した「3 状態ボタン」スプライト。
        let mut sprite = Pixmap::new(12, 4);
        sprite.fill([0, 0, 0, 0]);
        let red = Pixmap::from_rgba_straight(4, 4, &[255, 0, 0, 255].repeat(4 * 4));
        let green = Pixmap::from_rgba_straight(4, 4, &[0, 255, 0, 255].repeat(4 * 4));
        let blue = Pixmap::from_rgba_straight(4, 4, &[0, 0, 255, 255].repeat(4 * 4));
        sprite.blit_over(&red, 0, 0, 255);
        sprite.blit_over(&green, 4, 0, 255);
        sprite.blit_over(&blue, 8, 0, 255);
        save_png(&sprite, &dir.join("button.png")).unwrap();

        let es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        // 既存レイヤへの上書き（既存内容を消さない）を確認するため、先に透明キャンバスを確保。
        es.layer_fill(id, "fore", 4, 4, 0, 0, 0, 0).unwrap();
        // 中央 (over/緑) だけを clip して (0,0) へブリット。
        es.layer_blit_image(
            id,
            "fore",
            "button.png",
            0,
            0,
            Some(BlitClip {
                x: 4,
                y: 0,
                w: 4,
                h: 4,
            }),
        )
        .unwrap();

        let scene = es.scene.lock().unwrap();
        let layer = scene.layer(Page::Fore, id).expect("レイヤが存在するはず");
        let pixmap = layer.pixmap.as_ref().expect("blit 後は pixmap があるはず");
        assert_eq!((pixmap.width, pixmap.height), (4, 4));
        for px in pixmap.data.as_chunks::<4>().0 {
            assert_eq!(px, &[0, 255, 0, 255], "clip した緑マスのみが写るはず");
        }
        drop(scene);

        std::fs::remove_dir_all(&dir).ok();
    }

    fn temp_game_dir(name: &str) -> std::path::PathBuf {
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("noiria-{name}-{ns}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("game.json"),
            format!(r#"{{"title":"t","id":"noiria-{name}","main":"main.ts"}}"#),
        )
        .unwrap();
        dir
    }

    #[test]
    fn audio_play_accepts_finite_nonnegative_start_and_rejects_invalid_values() {
        let dir = temp_game_dir("audio-start-position-test");
        let es = make_engine_state(&dir);
        for start in [0.0, 2345.0, f64::MAX] {
            assert!(
                es.audio_play("bgm", "clip.wav", true, 1.0, 0, start, false)
                    .is_ok(),
                "有限の非負値は受理する: {start}"
            );
        }
        for start in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                es.audio_play("bgm", "clip.wav", true, 1.0, 0, start, false)
                    .is_err(),
                "異常値はエラーにする: {start}"
            );
        }
        assert_eq!(es.audio_position("bgm").unwrap(), u64::MAX);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn audio_play_exposes_requested_start_before_audio_thread_acknowledges_it() {
        let dir = temp_game_dir("audio-start-position-eager-test");
        let es = make_engine_state(&dir);
        es.audio_shared
            .mark_play_requested(noiria_core::audio::CH_BGM, 111);

        es.audio_play("bgm", "clip.wav", true, 1.0, 0, 2345.0, true)
            .unwrap();

        assert_eq!(es.audio_position("bgm").unwrap(), 2345);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// [stopquake]（W5）: quake_stop が scene.quake を即座にクリアすることを確認する。
    #[test]
    fn quake_stop_clears_active_quake() {
        let dir = temp_game_dir("quake-stop-test");
        let es = make_engine_state(&dir);
        es.quake(5, 5, 5000);
        assert!(
            es.scene.lock().unwrap().quake.is_some(),
            "quake 直後は Some のはず"
        );
        es.quake_stop();
        assert!(
            es.scene.lock().unwrap().quake.is_none(),
            "quake_stop 後は None のはず"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// [move path=]（W5）: layer_move_path が第一区間を tween、残りを tween_queue へ
    /// 積むことを確認する（区間をまたぐ継ぎ目のロジック自体は scene.rs の
    /// `settle_finished_tweens_chains_to_next_waypoint_seamlessly` 等で検証済み。
    /// ここでは ops 経由の配線・省略値の引き継ぎだけを確認する）。
    #[test]
    fn layer_move_path_starts_first_leg_and_queues_the_rest() {
        let dir = temp_game_dir("move-path-test");
        let es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        es.layer_move_path(
            id,
            "fore",
            vec![
                MoveArgs {
                    to_x: Some(50),
                    to_y: None,
                    to_opacity: None,
                    time_ms: 100,
                    accel: 0.0,
                },
                MoveArgs {
                    to_x: Some(150),
                    to_y: Some(80),
                    to_opacity: Some(128),
                    time_ms: 200,
                    accel: 0.5,
                },
            ],
            false,
        )
        .unwrap();
        let scene = es.scene.lock().unwrap();
        let layer = scene.layer(Page::Fore, id).unwrap();
        let t = layer
            .tween
            .as_ref()
            .expect("第一区間の tween が開始しているはず");
        assert_eq!(
            (t.to_x, t.to_y, t.to_opacity),
            (50, 0, 255),
            "第一区間: y/opacity は現在値を引き継ぐ"
        );
        assert!(t.spline.is_none());
        assert_eq!(
            layer.tween_queue.len(),
            1,
            "残り区間は tween_queue へ積まれるはず"
        );
        let wp = &layer.tween_queue[0];
        assert_eq!((wp.x, wp.y, wp.opacity), (150, 80, 128));
        assert!(wp.spline.is_none());
        drop(scene);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn layer_move_path_rejects_short_spline_and_attaches_segments_on_back_page() {
        let dir = temp_game_dir("move-spline-test");
        let es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        let short = vec![MoveArgs {
            to_x: Some(100),
            to_y: Some(100),
            to_opacity: None,
            time_ms: 100,
            accel: 0.0,
        }];
        assert!(es.layer_move_path(id, "back", short, true).is_err());
        assert!(es
            .scene
            .lock()
            .unwrap()
            .layer(Page::Back, id)
            .unwrap()
            .tween
            .is_none());

        es.layer_move_path(
            id,
            "back",
            vec![
                MoveArgs {
                    to_x: Some(100),
                    to_y: Some(100),
                    to_opacity: Some(100),
                    time_ms: 100,
                    accel: 0.0,
                },
                MoveArgs {
                    to_x: Some(0),
                    to_y: Some(0),
                    to_opacity: Some(200),
                    time_ms: 100,
                    accel: 0.0,
                },
            ],
            true,
        )
        .unwrap();

        let scene = es.scene.lock().unwrap();
        assert!(scene.layer(Page::Fore, id).unwrap().tween.is_none());
        let layer = scene.layer(Page::Back, id).unwrap();
        let first = layer.tween.as_ref().unwrap();
        assert!(first.spline.is_some());
        assert_eq!(first.current_x(first.start + Duration::from_millis(50)), 69);
        assert_eq!(
            first.current_opacity(first.start + Duration::from_millis(50)),
            178
        );
        assert_eq!(layer.tween_queue.len(), 1);
        assert!(layer.tween_queue[0].spline.is_some());
        drop(scene);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn layer_moving_includes_pending_queue_after_current_leg_finishes() {
        let dir = temp_game_dir("move-queue-moving-test");
        let es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        es.layer_move_path(
            id,
            "fore",
            vec![
                MoveArgs {
                    to_x: Some(100),
                    to_y: None,
                    to_opacity: None,
                    time_ms: 0,
                    accel: 0.0,
                },
                MoveArgs {
                    to_x: Some(200),
                    to_y: None,
                    to_opacity: None,
                    time_ms: 10_000,
                    accel: 0.0,
                },
            ],
            true,
        )
        .unwrap();
        {
            let scene = es.scene.lock().unwrap();
            let layer = scene.layer(Page::Fore, id).unwrap();
            assert!(layer.tween.as_ref().unwrap().is_finished(es.clock.now()));
            assert_eq!(layer.tween_queue.len(), 1);
        }
        assert!(es.layer_moving());
        es.layer_move_skip();
        assert!(!es.layer_moving());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn single_move_interrupts_spline_path_and_discards_pending_waypoints() {
        let dir = temp_game_dir("move-spline-interrupt-test");
        let es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        es.layer_move_path(
            id,
            "fore",
            vec![
                MoveArgs {
                    to_x: Some(100),
                    to_y: Some(100),
                    to_opacity: None,
                    time_ms: 10_000,
                    accel: 0.0,
                },
                MoveArgs {
                    to_x: Some(200),
                    to_y: Some(0),
                    to_opacity: None,
                    time_ms: 10_000,
                    accel: 0.0,
                },
            ],
            true,
        )
        .unwrap();
        es.layer_move(
            id,
            "fore",
            MoveArgs {
                to_x: Some(25),
                to_y: Some(50),
                to_opacity: Some(128),
                time_ms: 100,
                accel: 0.0,
            },
        )
        .unwrap();

        let scene = es.scene.lock().unwrap();
        let layer = scene.layer(Page::Fore, id).unwrap();
        assert!(layer.tween.as_ref().unwrap().spline.is_none());
        assert!(layer.tween_queue.is_empty());
        let end = layer.tween.as_ref().unwrap().start + Duration::from_millis(100);
        drop(scene);

        let mut scene = es.scene.lock().unwrap();
        scene.settle_finished_tweens(end);
        let layer = scene.layer(Page::Fore, id).unwrap();
        assert!(layer.tween.is_none());
        assert_eq!((layer.x, layer.y, layer.opacity), (25, 50, 128));
        drop(scene);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn distance_allocated_path_samples_effective_position_when_interrupting() {
        let dir = temp_game_dir("move-path-distance-interrupt-test");
        let mut es = make_engine_state(&dir);
        let base = Instant::now();
        es.clock = Clock::new_virtual(base);
        let id = es.layer_new("image").unwrap();
        es.layer_move(
            id,
            "fore",
            MoveArgs {
                to_x: Some(100),
                to_y: None,
                to_opacity: None,
                time_ms: 1000,
                accel: 0.0,
            },
        )
        .unwrap();
        es.clock.advance(Duration::from_millis(500));

        es.layer_move_path_by_distance(
            id,
            "fore",
            vec![
                MoveArgs {
                    to_x: Some(100),
                    to_y: None,
                    to_opacity: None,
                    time_ms: 7,
                    accel: 0.0,
                },
                MoveArgs {
                    to_x: Some(300),
                    to_y: None,
                    to_opacity: None,
                    time_ms: 9,
                    accel: 0.0,
                },
            ],
            true,
            2500.0,
        )
        .unwrap();

        let scene = es.scene.lock().unwrap();
        let layer = scene.layer(Page::Fore, id).unwrap();
        let first = layer.tween.as_ref().unwrap();
        assert_eq!(first.from_x, 50);
        assert_eq!(first.duration, Duration::from_millis(500));
        assert_eq!(layer.tween_queue[0].duration, Duration::from_millis(2000));
        drop(scene);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn distance_allocated_path_handles_zero_distance_and_inherited_coordinates() {
        let dir = temp_game_dir("move-path-distance-edge-test");
        let es = make_engine_state(&dir);
        let stationary_id = es.layer_new("image").unwrap();
        es.layer_move_path_by_distance(
            stationary_id,
            "fore",
            vec![
                MoveArgs {
                    to_opacity: Some(128),
                    ..MoveArgs::default()
                },
                MoveArgs {
                    to_opacity: Some(64),
                    ..MoveArgs::default()
                },
            ],
            false,
            300.0,
        )
        .unwrap();
        let inherited_id = es.layer_new("image").unwrap();
        es.layer_move_path_by_distance(
            inherited_id,
            "fore",
            vec![
                MoveArgs {
                    to_x: Some(100),
                    ..MoveArgs::default()
                },
                MoveArgs {
                    to_y: Some(100),
                    ..MoveArgs::default()
                },
            ],
            false,
            200.0,
        )
        .unwrap();

        let scene = es.scene.lock().unwrap();
        let stationary = scene.layer(Page::Fore, stationary_id).unwrap();
        assert_eq!(
            stationary.tween.as_ref().unwrap().duration,
            Duration::from_millis(150)
        );
        assert_eq!(
            stationary.tween_queue[0].duration,
            Duration::from_millis(150)
        );
        let inherited = scene.layer(Page::Fore, inherited_id).unwrap();
        assert_eq!(
            inherited.tween.as_ref().unwrap().duration,
            Duration::from_millis(100)
        );
        assert_eq!(
            inherited.tween_queue[0].duration,
            Duration::from_millis(100)
        );
        drop(scene);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn layer_move_cancel_discards_active_path_without_applying_its_endpoint() {
        let dir = temp_game_dir("move-path-cancel-test");
        let es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        es.layer_move_path(
            id,
            "fore",
            vec![
                MoveArgs {
                    to_x: Some(100),
                    to_y: Some(50),
                    to_opacity: None,
                    time_ms: 1000,
                    accel: 0.0,
                },
                MoveArgs {
                    to_x: Some(200),
                    to_y: Some(0),
                    to_opacity: None,
                    time_ms: 1000,
                    accel: 0.0,
                },
            ],
            true,
        )
        .unwrap();

        es.layer_move_cancel(id, "fore").unwrap();

        let scene = es.scene.lock().unwrap();
        let layer = scene.layer(Page::Fore, id).unwrap();
        assert!(layer.tween.is_none());
        assert!(layer.tween_queue.is_empty());
        assert_eq!((layer.x, layer.y), (0, 0));
        drop(scene);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn text_reflow_preserves_active_reveal_and_refreshes_glyph_cache() {
        let dir = temp_game_dir("text-reflow-reveal-test");
        let mut es = make_engine_state(&dir);
        let base = Instant::now();
        es.clock = Clock::new_virtual(base);
        let id = es.layer_new("text").unwrap();
        let mut style = TextStyle {
            box_w: 300,
            ..TextStyle::default()
        };
        es.text_style(id, style.clone());
        es.text_print(
            id,
            vec![TextSegment {
                text: "AB".to_string(),
                ..TextSegment::default()
            }],
            false,
            0.0,
        )
        .unwrap();
        es.text_print(
            id,
            vec![TextSegment {
                text: "CDE".to_string(),
                ..TextSegment::default()
            }],
            true,
            1.0,
        )
        .unwrap();

        let (before_reveal, before_raster) = {
            let scene = es.scene.lock().unwrap();
            let text = scene.layer(Page::Fore, id).unwrap().text.as_ref().unwrap();
            (text.reveal.clone(), text.raster.clone())
        };
        style.align = noiria_core::text::TextAlign::Right;
        es.text_style(id, style);
        es.text_reflow(id).unwrap();

        let scene = es.scene.lock().unwrap();
        let text = scene.layer(Page::Fore, id).unwrap().text.as_ref().unwrap();
        assert_eq!(text.reveal.from_glyph, before_reveal.from_glyph);
        assert_eq!(text.reveal.start, before_reveal.start);
        assert_eq!(text.reveal.cps, before_reveal.cps);
        assert!(!Arc::ptr_eq(&text.raster, &before_raster));
        assert_eq!(
            es.texts.glyph_counts.get(&id).copied(),
            Some(text.raster.glyphs.len())
        );
        drop(scene);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn layer_blit_image_creates_canvas_when_layer_has_no_pixmap_yet() {
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("noiria-blit-image-test2-{ns}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("game.json"),
            r#"{"title":"t","id":"noiria-blit-image-test2","main":"main.ts"}"#,
        )
        .unwrap();
        let mut img = Pixmap::new(3, 3);
        img.fill([10, 20, 30, 255]);
        save_png(&img, &dir.join("solid.png")).unwrap();

        let es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        // clip 省略 = 画像全体。pixmap 未確保のレイヤでも自動でキャンバスができる。
        es.layer_blit_image(id, "fore", "solid.png", 0, 0, None)
            .unwrap();

        let scene = es.scene.lock().unwrap();
        let layer = scene.layer(Page::Fore, id).unwrap();
        let pixmap = layer.pixmap.as_ref().unwrap();
        assert_eq!((pixmap.width, pixmap.height), (3, 3));
        drop(scene);

        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- 動画 ops (W6: openvideo/preparevideo/playvideo/pausevideo/resumevideo/rewindvideo) ----
    // W6 では JPEG エンコードした極小フレーム列を自前で AVI (MJPEG) コンテナへ詰めて実行時に
    // 動的生成していた（`image` クレートを dev-dependency として使用）。W7 でリポジトリに
    // 固定フィクスチャとして同梱する方式へ変更し、`noiria-app` の `image` dev-dependency を
    // 削除した（`docs/implementation-notes.md`「2026-07-11 W7」参照）。フィクスチャ自体は
    // 生成当時と全く同じロジック（16x16 JPEG フレーム + `write_mjpeg_avi`）で 1 度だけ作成
    // したバイト列（`tests/fixtures/clip_2frames.avi`: shade=40,220 の 2 フレーム。
    // `tests/fixtures/clip_5frames.avi`: shade=10,20,30,40,50 の 5 フレーム）。

    static FIXTURE_CLIP_2FRAMES: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/clip_2frames.avi"
    ));
    static FIXTURE_CLIP_5FRAMES: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/clip_5frames.avi"
    ));
    static FIXTURE_VP9_16X16: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../noiria-core/tests/fixtures/vp9_p2_10bit_yuv420_16x16_2f.ivf"
    ));

    struct ChangingFrameSource {
        cursor: usize,
        width: u32,
        height: u32,
    }

    impl ChangingFrameSource {
        fn new() -> Self {
            Self {
                cursor: 0,
                width: 2,
                height: 1,
            }
        }
    }

    impl VideoSource for ChangingFrameSource {
        fn width(&self) -> u32 {
            self.width
        }

        fn height(&self) -> u32 {
            self.height
        }

        fn fps(&self) -> f32 {
            240.0
        }

        fn next_frame(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
            let frame = match self.cursor {
                0 => {
                    self.width = 2;
                    self.height = 1;
                    [255, 0, 0, 255].repeat(2)
                }
                1 => {
                    self.width = 3;
                    self.height = 2;
                    [0, 255, 0, 255].repeat(6)
                }
                _ => return Ok(None),
            };
            self.cursor += 1;
            Ok(Some(frame))
        }

        fn rewind(&mut self) -> anyhow::Result<()> {
            self.cursor = 0;
            self.width = 2;
            self.height = 1;
            Ok(())
        }
    }

    struct PanickingVideoSource;

    impl VideoSource for PanickingVideoSource {
        fn width(&self) -> u32 {
            1
        }

        fn height(&self) -> u32 {
            1
        }

        fn fps(&self) -> f32 {
            30.0
        }

        fn next_frame(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
            panic!("intentional video worker failure")
        }

        fn rewind(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct IndexedVideoSource {
        cursor: usize,
        frame_count: usize,
        fps: f32,
        base: u8,
        delay: Duration,
        decoded: Arc<Mutex<Vec<usize>>>,
    }

    impl IndexedVideoSource {
        fn new(
            frame_count: usize,
            fps: f32,
            base: u8,
            delay: Duration,
            decoded: Arc<Mutex<Vec<usize>>>,
        ) -> Self {
            Self {
                cursor: 0,
                frame_count,
                fps,
                base,
                delay,
                decoded,
            }
        }
    }

    impl VideoSource for IndexedVideoSource {
        fn width(&self) -> u32 {
            1
        }

        fn height(&self) -> u32 {
            1
        }

        fn fps(&self) -> f32 {
            self.fps
        }

        fn next_frame(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
            if self.cursor >= self.frame_count {
                return Ok(None);
            }
            if !self.delay.is_zero() {
                std::thread::sleep(self.delay);
            }
            let index = self.cursor;
            self.cursor += 1;
            self.decoded.lock().unwrap().push(index);
            Ok(Some(vec![self.base.saturating_add(index as u8), 0, 0, 255]))
        }

        fn rewind(&mut self) -> anyhow::Result<()> {
            self.cursor = 0;
            Ok(())
        }

        fn seek_frame(&mut self, target: usize) -> anyhow::Result<()> {
            anyhow::ensure!(
                target <= self.frame_count,
                "test frame target out of range: {target}"
            );
            self.cursor = target;
            Ok(())
        }
    }

    struct BlockOnceVideoSource {
        cursor: usize,
        frame_count: usize,
        fps: f32,
        base: u8,
        blocked: bool,
        started: std::sync::mpsc::Sender<usize>,
        release: std::sync::mpsc::Receiver<()>,
        dropped: std::sync::mpsc::Sender<()>,
    }

    impl VideoSource for BlockOnceVideoSource {
        fn width(&self) -> u32 {
            1
        }

        fn height(&self) -> u32 {
            1
        }

        fn fps(&self) -> f32 {
            self.fps
        }

        fn next_frame(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
            if !self.blocked {
                self.blocked = true;
                self.started
                    .send(self.cursor)
                    .map_err(|e| anyhow::anyhow!("decode start signal failed: {e}"))?;
                self.release
                    .recv_timeout(Duration::from_secs(2))
                    .map_err(|e| anyhow::anyhow!("decode release signal failed: {e}"))?;
            }
            if self.cursor >= self.frame_count {
                return Ok(None);
            }
            let index = self.cursor;
            self.cursor += 1;
            Ok(Some(vec![self.base.saturating_add(index as u8), 0, 0, 255]))
        }

        fn rewind(&mut self) -> anyhow::Result<()> {
            self.cursor = 0;
            Ok(())
        }

        fn seek_frame(&mut self, target: usize) -> anyhow::Result<()> {
            anyhow::ensure!(
                target <= self.frame_count,
                "test frame target out of range: {target}"
            );
            self.cursor = target;
            Ok(())
        }
    }

    impl Drop for BlockOnceVideoSource {
        fn drop(&mut self) {
            let _ = self.dropped.send(());
        }
    }

    fn block_once_video_source(
        cursor: usize,
        frame_count: usize,
        fps: f32,
        base: u8,
    ) -> (
        BlockOnceVideoSource,
        std::sync::mpsc::Receiver<usize>,
        std::sync::mpsc::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    ) {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
        (
            BlockOnceVideoSource {
                cursor,
                frame_count,
                fps,
                base,
                blocked: false,
                started: started_tx,
                release: release_rx,
                dropped: dropped_tx,
            },
            started_rx,
            release_tx,
            dropped_rx,
        )
    }

    fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("condition was not met within {timeout:?}");
    }

    /// 固定フィクスチャ（2 フレーム、shade 40→220）を一時ゲームディレクトリへ書き出す。
    fn write_test_avi_2frames(dir: &std::path::Path, name: &str) {
        std::fs::write(dir.join(name), FIXTURE_CLIP_2FRAMES).unwrap();
    }

    /// 固定フィクスチャ（5 フレーム、shade 10,20,30,40,50）を一時ゲームディレクトリへ書き出す。
    fn write_test_avi_5frames(dir: &std::path::Path, name: &str) {
        std::fs::write(dir.join(name), FIXTURE_CLIP_5FRAMES).unwrap();
    }

    #[test]
    fn video_open_does_not_touch_layer_pixmap_or_start_playback() {
        let dir = temp_game_dir("video-open-test");
        write_test_avi_2frames(&dir, "clip.avi");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        es.video_open(id, "clip.avi", false).unwrap();
        {
            let scene = es.scene.lock().unwrap();
            let layer = scene.layer(Page::Fore, id).unwrap();
            assert!(
                layer.pixmap.is_none(),
                "video_open だけではレイヤへ書き込まないはず"
            );
        }
        assert!(
            !es.video_is_playing(),
            "video_open だけでは再生は始まらない"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_prepare_shows_first_frame_and_does_not_start_playback() {
        let dir = temp_game_dir("video-prepare-test");
        write_test_avi_2frames(&dir, "clip.avi");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        es.video_prepare(id, "clip.avi", false).unwrap();
        {
            let scene = es.scene.lock().unwrap();
            let layer = scene.layer(Page::Fore, id).unwrap();
            let pixmap = layer
                .pixmap
                .as_ref()
                .expect("先頭フレームが表示されているはず");
            let center = ((pixmap.height / 2 * pixmap.width + pixmap.width / 2) * 4) as usize;
            assert!(
                (pixmap.data[center] as i32 - 40).abs() <= 12,
                "1 枚目 (shade=40) が表示されているはず: R={}",
                pixmap.data[center]
            );
        }
        assert!(!es.video_is_playing(), "prepare だけでは再生は始まらない");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_prepare_uses_decoded_frame_dimensions() {
        let dir = temp_game_dir("video-prepare-frame-size-test");
        let mut ivf = FIXTURE_VP9_16X16.to_vec();
        ivf[12..14].copy_from_slice(&1u16.to_le_bytes());
        ivf[14..16].copy_from_slice(&1u16.to_le_bytes());
        std::fs::write(dir.join("clip.ivf"), ivf).unwrap();
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();

        es.video_prepare(id, "clip.ivf", false).unwrap();

        let scene = es.scene.lock().unwrap();
        let pixmap = scene
            .layer(Page::Fore, id)
            .unwrap()
            .pixmap
            .as_ref()
            .unwrap();
        assert_eq!((pixmap.width, pixmap.height), (16, 16));
        assert_eq!(pixmap.data.len(), 16 * 16 * 4);
        drop(scene);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_prepared_seek_prepare_latch_period_and_segment_loop_are_exact() {
        let dir = temp_game_dir("video-b4-prepared-test");
        write_test_avi_2frames(&dir, "clip.avi");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();

        es.video_open(id, "clip.avi", false).unwrap();
        es.video_seek_frame(1).unwrap();
        es.video_set_period_event(1);
        es.video_set_segment_loop(0, 2).unwrap();
        es.video_prepare_opened(id).unwrap();

        {
            let scene = es.scene.lock().unwrap();
            let pixmap = scene
                .layer(Page::Fore, id)
                .unwrap()
                .pixmap
                .as_ref()
                .unwrap();
            let center = ((pixmap.height / 2 * pixmap.width + pixmap.width / 2) * 4) as usize;
            assert!(
                (pixmap.data[center] as i32 - 220).abs() <= 12,
                "seek target frame was not used for prepare preview: {}",
                pixmap.data[center]
            );
        }
        assert!(es.video_take_event("prepare"));
        assert!(
            !es.video_take_event("prepare"),
            "prepare latch must be one-shot"
        );

        es.video_play_prepared(id).unwrap();
        let current_events = es.videos.current.as_ref().unwrap().events.clone();
        wait_until(Duration::from_secs(2), || es.video_take_event("period"));
        wait_until(Duration::from_secs(2), || {
            current_events.lock().unwrap().seg_loop_count >= 2
        });
        assert!(
            !es.video_take_event("period"),
            "period event must remain cleared after crossing the target again"
        );

        es.video_cancel_segment_loop();
        wait_until(Duration::from_secs(2), || !es.video_is_playing());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_position_seek_updates_prepared_preview() {
        let dir = temp_game_dir("video-b4-position-test");
        write_test_avi_2frames(&dir, "clip.avi");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        es.video_open(id, "clip.avi", false).unwrap();
        let fps = es.videos.prepared.as_ref().unwrap().source.fps() as f64;

        assert_eq!(video_optional_frame(-1.0).unwrap(), None);
        assert_eq!(
            video_optional_frame(u32::MAX as f64).unwrap(),
            Some(u32::MAX as usize)
        );
        assert!(video_optional_frame(-2.0).is_err());
        assert!(video_optional_frame(1.5).is_err());
        assert!(video_optional_frame(u32::MAX as f64 + 1.0).is_err());
        assert!(es.video_seek_position(-1.0).is_err());
        assert!(es.video_seek_position(f64::NAN).is_err());
        assert!(es.video_seek_position(f64::INFINITY).is_err());
        es.video_seek_position(1500.0 / fps).unwrap();
        es.video_prepare_opened(id).unwrap();

        let scene = es.scene.lock().unwrap();
        let pixmap = scene
            .layer(Page::Fore, id)
            .unwrap()
            .pixmap
            .as_ref()
            .unwrap();
        let center = ((pixmap.height / 2 * pixmap.width + pixmap.width / 2) * 4) as usize;
        assert!(
            (pixmap.data[center] as i32 - 220).abs() <= 12,
            "position seek preview did not select frame 1: {}",
            pixmap.data[center]
        );
        drop(scene);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_commit_gate_drops_decoded_frame_when_seek_returns() {
        let dir = temp_game_dir("video-b4-seek-race-test");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        let events = Arc::new(Mutex::new(VideoEvents::default()));
        let (source, started, release, dropped) = block_once_video_source(0, 3, 240.0, 10);

        es.spawn_video_playback(
            id,
            Box::new(source),
            false,
            0,
            Arc::new(Mutex::new(PlaybackControl::default())),
            events.clone(),
        );
        assert_eq!(started.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
        es.video_seek_frame(2).unwrap();
        release.send(()).unwrap();
        dropped.recv_timeout(Duration::from_secs(2)).unwrap();

        let events = events.lock().unwrap();
        assert_eq!(events.displayed_count, 1);
        assert_eq!(events.last_displayed_frame, Some(2));
        drop(events);
        let scene = es.scene.lock().unwrap();
        assert_eq!(
            scene
                .layer(Page::Fore, id)
                .unwrap()
                .pixmap
                .as_ref()
                .unwrap()
                .data,
            [12, 0, 0, 255]
        );
        drop(scene);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_commit_gate_observes_period_set_during_decode() {
        let dir = temp_game_dir("video-b4-period-race-test");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        let events = Arc::new(Mutex::new(VideoEvents::default()));
        let (source, started, decode_release, dropped) = block_once_video_source(1, 2, 240.0, 20);
        let (commit_reached_tx, commit_reached_rx) = std::sync::mpsc::channel();
        let (commit_release_tx, commit_release_rx) = std::sync::mpsc::channel();
        let control = Arc::new(Mutex::new(PlaybackControl::default()));
        control.lock().unwrap().before_commit_hook = Some(VideoCommitHook {
            reached: commit_reached_tx,
            release: commit_release_rx,
        });

        es.spawn_video_playback(id, Box::new(source), false, 1, control, events.clone());
        assert_eq!(started.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        decode_release.send(()).unwrap();
        commit_reached_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        es.video_set_period_event(1);
        commit_release_tx.send(()).unwrap();
        wait_until(Duration::from_secs(2), || es.video_take_event("period"));
        dropped.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(events.lock().unwrap().last_displayed_frame, Some(1));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_commit_gate_rechecks_segment_set_during_decode() {
        let dir = temp_game_dir("video-b4-segment-race-test");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        let events = Arc::new(Mutex::new(VideoEvents::default()));
        let (source, started, release, dropped) = block_once_video_source(3, 4, 1.0, 30);

        es.spawn_video_playback(
            id,
            Box::new(source),
            false,
            3,
            Arc::new(Mutex::new(PlaybackControl::default())),
            events.clone(),
        );
        assert_eq!(started.recv_timeout(Duration::from_secs(2)).unwrap(), 3);
        es.video_set_segment_loop(1, 3).unwrap();
        release.send(()).unwrap();
        wait_until(Duration::from_secs(2), || {
            events.lock().unwrap().displayed_count >= 1
        });
        {
            let events = events.lock().unwrap();
            assert_eq!(events.displayed_count, 1);
            assert_eq!(events.last_displayed_frame, Some(1));
            assert_eq!(events.seg_loop_count, 1);
        }
        let scene = es.scene.lock().unwrap();
        assert_eq!(
            scene
                .layer(Page::Fore, id)
                .unwrap()
                .pixmap
                .as_ref()
                .unwrap()
                .data,
            [31, 0, 0, 255]
        );
        drop(scene);
        es.video_stop();
        dropped.recv_timeout(Duration::from_secs(2)).unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_eof_gate_honors_seek_set_during_eof_read() {
        let dir = temp_game_dir("video-b4-eof-race-test");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        let events = Arc::new(Mutex::new(VideoEvents::default()));
        let (source, started, release, dropped) = block_once_video_source(2, 2, 240.0, 40);

        es.spawn_video_playback(
            id,
            Box::new(source),
            false,
            2,
            Arc::new(Mutex::new(PlaybackControl::default())),
            events.clone(),
        );
        assert_eq!(started.recv_timeout(Duration::from_secs(2)).unwrap(), 2);
        es.video_seek_frame(1).unwrap();
        release.send(()).unwrap();
        dropped.recv_timeout(Duration::from_secs(2)).unwrap();

        let events = events.lock().unwrap();
        assert_eq!(events.displayed_count, 1);
        assert_eq!(events.last_displayed_frame, Some(1));
        drop(events);
        let scene = es.scene.lock().unwrap();
        assert_eq!(
            scene
                .layer(Page::Fore, id)
                .unwrap()
                .pixmap
                .as_ref()
                .unwrap()
                .data,
            [41, 0, 0, 255]
        );
        drop(scene);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_stop_is_inactive_before_blocked_decode_returns() {
        let dir = temp_game_dir("video-b4-stop-race-test");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        let events = Arc::new(Mutex::new(VideoEvents::default()));
        let (source, started, release, dropped) = block_once_video_source(0, 1, 240.0, 50);

        es.spawn_video_playback(
            id,
            Box::new(source),
            false,
            0,
            Arc::new(Mutex::new(PlaybackControl::default())),
            events.clone(),
        );
        assert_eq!(started.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
        es.video_stop();
        assert!(!es.video_is_playing());
        release.send(()).unwrap();
        dropped.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(events.lock().unwrap().displayed_count, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_segment_loop_is_end_exclusive_and_cancel_resumes_to_eof() {
        let dir = temp_game_dir("video-b4-segment-test");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        let decoded = Arc::new(Mutex::new(Vec::new()));
        let control = Arc::new(Mutex::new(PlaybackControl::default()));
        control.lock().unwrap().segment_loop = Some((1, 4));
        let events = Arc::new(Mutex::new(VideoEvents::default()));

        es.spawn_video_playback(
            id,
            Box::new(IndexedVideoSource::new(
                5,
                240.0,
                10,
                Duration::ZERO,
                decoded.clone(),
            )),
            false,
            0,
            control,
            events.clone(),
        );
        wait_until(Duration::from_secs(2), || {
            events.lock().unwrap().seg_loop_count > 0 && decoded.lock().unwrap().len() >= 8
        });
        es.video_cancel_segment_loop();
        wait_until(Duration::from_secs(2), || !es.video_is_playing());

        let sequence = decoded.lock().unwrap().clone();
        assert_eq!(&sequence[..7], &[0, 1, 2, 3, 1, 2, 3]);
        assert_eq!(sequence.last(), Some(&4));
        assert!(
            sequence[..sequence.len() - 1]
                .iter()
                .all(|&frame| frame < 4),
            "segment end frame was decoded before cancellation: {sequence:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_natural_loop_event_fires_at_each_eof_boundary() {
        let dir = temp_game_dir("video-b4-loop-event-test");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        es.spawn_video_playback(
            id,
            Box::new(IndexedVideoSource::new(
                2,
                120.0,
                30,
                Duration::ZERO,
                Arc::new(Mutex::new(Vec::new())),
            )),
            true,
            0,
            Arc::new(Mutex::new(PlaybackControl::default())),
            Arc::new(Mutex::new(VideoEvents::default())),
        );

        wait_until(Duration::from_secs(2), || es.video_take_event("loop"));
        assert!(
            !es.video_take_event("loop"),
            "a loop boundary must produce one latch"
        );
        wait_until(Duration::from_secs(2), || es.video_take_event("loop"));

        es.video_stop();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_event_registration_and_cancel_clear_stale_latches() {
        let dir = temp_game_dir("video-b4-event-clear-test");
        write_test_avi_2frames(&dir, "clip.avi");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        es.video_open(id, "clip.avi", false).unwrap();
        let events = es.videos.prepared.as_ref().unwrap().events.clone();

        events.lock().unwrap().signal(VideoEventKind::Period);
        events.lock().unwrap().signal(VideoEventKind::SegLoop);
        es.video_set_period_event(1);
        es.video_set_segment_loop(0, 2).unwrap();
        assert!(!es.video_take_event("period"));
        assert!(!es.video_take_event("segLoop"));

        events.lock().unwrap().signal(VideoEventKind::Period);
        events.lock().unwrap().signal(VideoEventKind::SegLoop);
        es.video_cancel_period_event();
        es.video_cancel_segment_loop();
        assert!(!es.video_take_event("period"));
        assert!(!es.video_take_event("segLoop"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_pause_freezes_display_and_events_until_resume() {
        let dir = temp_game_dir("video-b4-pause-test");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        let events = Arc::new(Mutex::new(VideoEvents::default()));
        es.spawn_video_playback(
            id,
            Box::new(IndexedVideoSource::new(
                2,
                120.0,
                20,
                Duration::ZERO,
                Arc::new(Mutex::new(Vec::new())),
            )),
            true,
            0,
            Arc::new(Mutex::new(PlaybackControl::default())),
            events.clone(),
        );
        wait_until(Duration::from_secs(2), || {
            events.lock().unwrap().displayed_count >= 2
        });

        es.video_pause();
        es.video_seek_position(1500.0 / 120.0).unwrap();
        let current = es.videos.current.as_ref().unwrap();
        assert_eq!(current.control.lock().unwrap().pending_seek, Some(1));
        let before = {
            let events = events.lock().unwrap();
            (
                events.displayed_count,
                events.loop_count,
                events.period_count,
                events.seg_loop_count,
            )
        };
        std::thread::sleep(Duration::from_millis(80));
        let during = {
            let events = events.lock().unwrap();
            (
                events.displayed_count,
                events.loop_count,
                events.period_count,
                events.seg_loop_count,
            )
        };
        assert_eq!(during, before, "pause advanced a frame or video event");

        es.video_resume();
        wait_until(Duration::from_secs(2), || {
            events.lock().unwrap().displayed_count > before.0
        });
        es.video_stop();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stale_video_worker_cannot_draw_or_signal_into_new_playback() {
        let dir = temp_game_dir("video-b4-generation-test");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        let old_control = Arc::new(Mutex::new(PlaybackControl::default()));
        old_control.lock().unwrap().period_frame = Some(0);
        let old_events = Arc::new(Mutex::new(VideoEvents::default()));

        es.spawn_video_playback(
            id,
            Box::new(IndexedVideoSource::new(
                1,
                30.0,
                10,
                Duration::from_millis(80),
                Arc::new(Mutex::new(Vec::new())),
            )),
            false,
            0,
            old_control,
            old_events.clone(),
        );
        let old_playing = es.videos.current.as_ref().unwrap().playing.clone();
        std::thread::sleep(Duration::from_millis(5));

        es.spawn_video_playback(
            id,
            Box::new(IndexedVideoSource::new(
                1,
                120.0,
                200,
                Duration::ZERO,
                Arc::new(Mutex::new(Vec::new())),
            )),
            false,
            0,
            Arc::new(Mutex::new(PlaybackControl::default())),
            Arc::new(Mutex::new(VideoEvents::default())),
        );
        wait_until(Duration::from_secs(2), || !es.video_is_playing());
        wait_until(Duration::from_secs(2), || {
            !old_playing.load(Ordering::Relaxed)
        });

        let scene = es.scene.lock().unwrap();
        let pixmap = scene
            .layer(Page::Fore, id)
            .unwrap()
            .pixmap
            .as_ref()
            .unwrap();
        assert_eq!(pixmap.data, [200, 0, 0, 255]);
        drop(scene);
        assert_eq!(old_events.lock().unwrap().period_count, 0);
        assert!(!es.video_take_event("period"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_playback_uses_each_frame_dimensions() {
        let dir = temp_game_dir("video-changing-frame-size-test");
        let es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        let playing = Arc::new(AtomicBool::new(true));

        playback_loop(
            es.scene.clone(),
            es.wake.clone(),
            id,
            Box::new(ChangingFrameSource::new()),
            false,
            0,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(PlaybackControl::default())),
            Arc::new(Mutex::new(VideoEvents::default())),
            Arc::new(AtomicU64::new(1)),
            1,
            playing.clone(),
        );

        assert!(!playing.load(Ordering::Relaxed));
        let scene = es.scene.lock().unwrap();
        for page in [Page::Fore, Page::Back] {
            let pixmap = scene.layer(page, id).unwrap().pixmap.as_ref().unwrap();
            assert_eq!((pixmap.width, pixmap.height), (3, 2));
            assert_eq!(pixmap.data, [0, 255, 0, 255].repeat(6));
        }
        drop(scene);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_playback_clears_playing_when_worker_panics() {
        let (wake_tx, _wake_rx) = unbounded();
        let playing = Arc::new(AtomicBool::new(true));
        let worker_playing = playing.clone();

        let result = std::thread::spawn(move || {
            playback_loop(
                Arc::new(Mutex::new(Scene::new(1, 1))),
                WakeSink::Channel(wake_tx),
                1,
                Box::new(PanickingVideoSource),
                false,
                0,
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicBool::new(false)),
                Arc::new(Mutex::new(PlaybackControl::default())),
                Arc::new(Mutex::new(VideoEvents::default())),
                Arc::new(AtomicU64::new(1)),
                1,
                worker_playing,
            )
        })
        .join();

        assert!(result.is_err());
        assert!(!playing.load(Ordering::Relaxed));
    }

    /// [pausevideo]/[resumevideo]/[rewindvideo] の状態遷移: 再生開始→pause→resume→rewind→stop。
    #[test]
    fn video_play_prepared_then_pause_resume_rewind_stop_transitions() {
        let dir = temp_game_dir("video-pause-test");
        write_test_avi_5frames(&dir, "clip.avi");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        es.video_prepare(id, "clip.avi", true).unwrap();
        es.video_play_prepared(id).unwrap();

        let mut waited = 0;
        while !es.video_is_playing() && waited < 500 {
            std::thread::sleep(Duration::from_millis(10));
            waited += 10;
        }
        assert!(es.video_is_playing(), "再生開始後は isPlaying のはず");
        assert!(!es.video_is_paused(), "開始直後は isPaused=false のはず");

        es.video_pause();
        assert!(
            es.video_is_paused(),
            "pause 直後は isPaused のはず（フラグは同期的に立つ）"
        );
        std::thread::sleep(Duration::from_millis(80));
        assert!(
            es.video_is_playing(),
            "pause 中もスレッドは生存し isPlaying=true のまま"
        );

        es.video_resume();
        assert!(!es.video_is_paused(), "resume 後は isPaused=false のはず");

        es.video_rewind(); // 巻き戻し要求がパニックせず受理されることを確認する

        es.video_stop();
        let mut waited = 0;
        while es.video_is_playing() && waited < 2000 {
            std::thread::sleep(Duration::from_millis(20));
            waited += 20;
        }
        assert!(
            !es.video_is_playing(),
            "stop 後はスレッドが終了して isPlaying=false になるはず"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_play_prepared_errors_when_nothing_prepared() {
        let dir = temp_game_dir("video-play-prepared-empty-test");
        let mut es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        assert!(
            es.video_play_prepared(id).is_err(),
            "準備済みが無ければエラーになるはず"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_play_prepared_errors_when_layer_mismatch() {
        let dir = temp_game_dir("video-play-prepared-mismatch-test");
        write_test_avi_2frames(&dir, "clip.avi");
        let mut es = make_engine_state(&dir);
        let id_a = es.layer_new("image").unwrap();
        let id_b = es.layer_new("image").unwrap();
        es.video_open(id_a, "clip.avi", false).unwrap();
        assert!(
            es.video_play_prepared(id_b).is_err(),
            "準備済みと異なるレイヤ指定はエラーになるはず"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 重い（半透明の全面レイヤが何枚もある）シーンを作り、合成 1 回の所要時間を返す。
    fn make_heavy_scene(es: &EngineState) -> Duration {
        let mut scene = Scene::new(2048, 1152);
        for k in 0..6u8 {
            let id = scene.alloc_layer(LayerKind::Image);
            let mut p = Pixmap::new(2048, 1152);
            p.fill([40 * k, 90, 200, 255]);
            let layer = scene.layer_mut(Page::Fore, id).unwrap();
            layer.pixmap = Some(Arc::new(p));
            layer.visible = true;
            layer.opacity = 128;
        }
        let mut bufs = ComposeBufs::new(2048, 1152);
        let mut out = Pixmap::new(2048, 1152);
        let t = Instant::now();
        compose_frame(&scene, Instant::now(), &mut bufs, &mut out);
        let one = t.elapsed();
        *es.scene.lock().unwrap() = scene;
        one
    }

    /// `screenshot` / `thumb_capture` は、合成・PNG 書き込みのあいだシーンのロックを握らない
    /// （握ると描画スレッドと他の ops がその間ずっと待たされる）。別スレッドが `scene.lock()` するときの
    /// 最大待ち時間が、合成 1 回ぶんより十分短いことで確かめる。
    #[test]
    fn screenshot_and_thumb_capture_do_not_hold_the_scene_lock_while_composing() {
        let dir = temp_game_dir("lock-free-capture-test");
        let es = make_engine_state(&dir);
        let compose = make_heavy_scene(&es);
        assert!(
            compose >= Duration::from_millis(5),
            "検証用のシーンが軽すぎる（合成 {compose:?}）"
        );
        let stop = Arc::new(AtomicBool::new(false));
        let observer = {
            let (scene, stop) = (es.scene.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut worst = Duration::ZERO;
                while !stop.load(Ordering::SeqCst) {
                    let t = Instant::now();
                    drop(scene.lock().unwrap());
                    worst = worst.max(t.elapsed());
                    std::thread::sleep(Duration::from_micros(200));
                }
                worst
            })
        };
        for _ in 0..4 {
            es.thumb_capture_sized(0, 0).unwrap();
            es.screenshot("r2-lock-test").unwrap();
        }
        stop.store(true, Ordering::SeqCst);
        let worst = observer.join().unwrap();
        eprintln!("SCENE_LOCK_WORST_WAIT_MS={:.2} COMPOSE_MS={:.2}", worst.as_secs_f64() * 1e3, compose.as_secs_f64() * 1e3);
        assert!(
            worst < compose / 3,
            "シーンのロックを最大 {worst:?} 待たされた（合成 1 回 {compose:?}）"
        );
        let _ = savedata::delete_slot(&es.config.id, "r2-lock-test");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 巨大な寸法はエラーで返り、その後も同じ状態から動く（`layer_fill` が 40GB を確保して abort しない）。
    #[test]
    fn oversized_dimensions_are_errors_not_allocations() {
        let dir = temp_game_dir("oversized-dims-test");
        let es = make_engine_state(&dir);
        let id = es.layer_new("image").unwrap();
        assert!(es.layer_fill(id, "fore", 100_000, 100_000, 0, 0, 0, 255).is_err());
        assert!(es.layer_fill(id, "fore", 1 << 31, 1 << 31, 0, 0, 0, 255).is_err());
        assert!(es.layer_msgbox(id, "fore", 100_000, 100_000, 4, 0, 0, 0, 255).is_err());
        assert!(es.thumb_capture_sized(100_000, 100_000).is_err());
        assert!(es.layer_fill(id, "fore", 8, 8, 0, 0, 0, 255).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 別スレッドがシーンのロックを握ったまま panic して毒化しても、ops は続けて動く
    /// （`PoisonError` の unwrap で連鎖して、本当のエラーが埋もれない）。
    #[test]
    fn a_poisoned_scene_lock_does_not_cascade_into_later_ops() {
        let dir = temp_game_dir("poisoned-lock-test");
        let es = make_engine_state(&dir);
        let scene = es.scene.clone();
        let panicked = std::thread::spawn(move || {
            let _guard = scene.lock().unwrap();
            panic!("シーンのロックを握ったまま落ちる");
        })
        .join();
        assert!(panicked.is_err() && es.scene.is_poisoned());
        let id = es.layer_new("image").unwrap();
        es.layer_fill(id, "fore", 4, 4, 1, 2, 3, 255).unwrap();
        es.layer_set_props(id, "fore", LayerProps { x: Some(3), ..LayerProps::default() }).unwrap();
        assert!(!es.layer_moving());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn next_video_pixmap_reuses_unreferenced_buffer_and_protects_shared_frames() {
        let mut src = noiria_core::codec::video::open_video(FIXTURE_CLIP_5FRAMES.to_vec()).unwrap();
        let mut recycle: Vec<Arc<Pixmap>> = Vec::new();

        let f0 = next_video_pixmap(src.as_mut(), &mut recycle).unwrap().unwrap();
        let f0_data_ptr = f0.data.as_ptr();
        let f0_cap = f0.data.capacity();
        let f0_snapshot = f0.data.clone();
        let mut held_by_scene = f0.clone();
        push_recycle(&mut recycle, f0);

        let f1 = next_video_pixmap(src.as_mut(), &mut recycle).unwrap().unwrap();
        assert!(!Arc::ptr_eq(&held_by_scene, &f1));
        assert_ne!(f1.data.as_ptr(), f0_data_ptr);
        assert_eq!(held_by_scene.data, f0_snapshot);

        held_by_scene = f1.clone();
        push_recycle(&mut recycle, f1);

        let f2 = next_video_pixmap(src.as_mut(), &mut recycle).unwrap().unwrap();
        assert_eq!(f2.data.as_ptr(), f0_data_ptr);
        assert_eq!(f2.data.capacity(), f0_cap);
        assert!(!Arc::ptr_eq(&held_by_scene, &f2));
        assert_ne!(f2.data, f0_snapshot);
    }
}
