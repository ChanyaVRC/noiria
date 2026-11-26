//! 動画再生ワーカー・フレーム再利用・`EngineState` の動画制御メソッド。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use noiria_core::codec::video::VideoSource;
use noiria_core::pixmap::Pixmap;
use noiria_core::scene::Page;

use crate::bridge::{LockExt, SharedScene, UserEvent, WakeSink};

use super::{EngineError, EngineState};

#[derive(Default)]
pub(super) struct PlaybackControl {
    pub(super) pending_seek: Option<usize>,
    pub(super) period_frame: Option<usize>,
    pub(super) segment_loop: Option<(usize, usize)>,
    segment_revision: Arc<AtomicU64>,
    #[cfg(test)]
    pub(super) before_commit_hook: Option<VideoCommitHook>,
}

#[cfg(test)]
pub(super) struct VideoCommitHook {
    pub(super) reached: std::sync::mpsc::Sender<()>,
    pub(super) release: std::sync::mpsc::Receiver<()>,
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
pub(super) enum VideoEventKind {
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
pub(super) struct VideoEvents {
    pub(super) loop_count: u64,
    pub(super) period_count: u64,
    pub(super) prepare_count: u64,
    pub(super) seg_loop_count: u64,
    pub(super) displayed_count: u64,
    pub(super) last_displayed_frame: Option<usize>,
}

impl VideoEvents {
    fn count_mut(&mut self, kind: VideoEventKind) -> &mut u64 {
        match kind {
            VideoEventKind::Loop => &mut self.loop_count,
            VideoEventKind::Period => &mut self.period_count,
            VideoEventKind::Prepare => &mut self.prepare_count,
            VideoEventKind::SegLoop => &mut self.seg_loop_count,
        }
    }

    pub(super) fn signal(&mut self, kind: VideoEventKind) {
        let count = self.count_mut(kind);
        *count = count.saturating_add(1);
    }

    fn take(&mut self, kind: VideoEventKind) -> bool {
        let count = self.count_mut(kind);
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
        *self.count_mut(kind) = 0;
    }
}

/// 再生中の動画を止めるためのハンドル（旧 `ops_video::VideoHandle`）。
pub(crate) struct VideoHandle {
    pub(super) stop: Arc<AtomicBool>,
    pub(super) paused: Arc<AtomicBool>,
    pub(super) playing: Arc<AtomicBool>,
    pub(super) control: Arc<Mutex<PlaybackControl>>,
    pub(super) events: Arc<Mutex<VideoEvents>>,
    fps: f64,
}

/// [openvideo]/[preparevideo]（W6）: 再生スレッドをまだ起動していない、開いただけの動画。
/// `playvideo` がこれを取り出して再生スレッドを起動する。
pub(crate) struct PreparedVideo {
    pub(crate) id: u32,
    pub(crate) source: Box<dyn VideoSource>,
    pub(crate) looping: bool,
    initial_index: usize,
    control: Arc<Mutex<PlaybackControl>>,
    pub(super) events: Arc<Mutex<VideoEvents>>,
}

/// 動画再生状態。同時再生は 1 本のみを想定（旧 `ops_video::Videos`）。
pub(crate) struct Videos {
    pub(crate) current: Option<Box<VideoHandle>>,
    /// [openvideo]/[preparevideo] で開いたが、まだ [playvideo] していない動画（W6）。
    pub(crate) prepared: Option<Box<PreparedVideo>>,
    generation: Arc<AtomicU64>,
}

impl Videos {
    pub(crate) fn new() -> Self {
        Self {
            current: None,
            prepared: None,
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    fn for_each_control_and_events(
        &self,
        mut f: impl FnMut(&Mutex<PlaybackControl>, &Mutex<VideoEvents>),
    ) {
        if let Some(prepared) = self.prepared.as_ref() {
            f(&prepared.control, &prepared.events);
        }
        if let Some(current) = self.current.as_ref() {
            f(&current.control, &current.events);
        }
    }
}

impl Default for Videos {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Videos {
    fn drop(&mut self) {
        if let Some(current) = self.current.as_ref() {
            current.stop.store(true, Ordering::Relaxed);
            current.playing.store(false, Ordering::Relaxed);
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

struct PlaybackStateGuard(Arc<AtomicBool>);

impl Drop for PlaybackStateGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

const MAX_RECYCLE_PIXMAPS: usize = 4;

pub(super) fn push_recycle(recycle: &mut Vec<Arc<Pixmap>>, pixmap: Arc<Pixmap>) {
    if recycle.len() >= MAX_RECYCLE_PIXMAPS {
        recycle.remove(0);
    }
    recycle.push(pixmap);
}

pub(super) fn next_video_pixmap(
    source: &mut dyn VideoSource,
    recycle: &mut Vec<Arc<Pixmap>>,
) -> anyhow::Result<Option<Arc<Pixmap>>> {
    // Arc::get_mut は Scene や ComposeBufs のキャッシュが参照を手放した後にだけ Some を返すので、
    // Arc::ptr_eq ベースの差分検出と衝突せずバッファを再利用できる。
    let reusable = recycle
        .iter_mut()
        .position(|p| Arc::get_mut(p).is_some())
        .map(|i| recycle.swap_remove(i));
    if let Some(mut arc) = reusable {
        if let Some(pm) = Arc::get_mut(&mut arc) {
            if !source.next_frame_into(&mut pm.data)? {
                recycle.push(arc);
                return Ok(None);
            }
            let (w, h) = (source.width(), source.height());
            assert_eq!(pm.data.len(), (w as usize) * (h as usize) * 4);
            debug_assert!(pm.data.as_chunks::<4>().0.iter().all(|px| px[3] == 255));
            pm.width = w;
            pm.height = h;
            return Ok(Some(arc));
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

pub(super) fn video_frame_at_position(fps: f64, position_ms: f64) -> Result<usize, EngineError> {
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

pub(super) struct PlaybackJob {
    pub(super) scene: SharedScene,
    pub(super) wake: WakeSink,
    pub(super) id: u32,
    pub(super) source: Box<dyn VideoSource>,
    pub(super) looping: bool,
    pub(super) initial_index: usize,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) paused: Arc<AtomicBool>,
    pub(super) control: Arc<Mutex<PlaybackControl>>,
    pub(super) events: Arc<Mutex<VideoEvents>>,
    pub(super) generation: Arc<AtomicU64>,
    pub(super) playback_generation: u64,
    pub(super) playing: Arc<AtomicBool>,
}

/// 動画再生スレッド本体。fps 間隔で `next_frame()` を呼び、対象レイヤ
/// (fore/back 両方) の pixmap を差し替えて再描画を要求する。
pub(super) fn playback_loop(job: PlaybackJob) {
    let PlaybackJob {
        scene,
        wake,
        id,
        mut source,
        looping,
        initial_index,
        stop,
        paused,
        control,
        events,
        generation,
        playback_generation,
        playing,
    } = job;
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
    let mut looped_without_frame = false;
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
            looped_without_frame = false;
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
                Ok(Some(pixmap)) => {
                    looped_without_frame = false;
                    Some(pixmap)
                }
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
                        if !retry && (!looping || looped_without_frame) {
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
                        if looped_without_frame {
                            log::warn!(
                                "ループ動画が先頭へ巻き戻してもフレームを返さないため再生を終了します"
                            );
                            break 'playback;
                        }
                        if let Err(e) = source.seek_frame(0) {
                            log::error!("動画の巻き戻しに失敗: {e:#}");
                            break 'playback;
                        }
                        index = 0;
                        looped_without_frame = true;
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
                } else if let Some(pixmap) = pending_pixmap.as_ref() {
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
                } else {
                    false
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

impl EngineState {
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
    pub(super) fn spawn_video_playback(
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
        self.videos.current = Some(Box::new(VideoHandle {
            stop: stop.clone(),
            paused: paused.clone(),
            playing: playing.clone(),
            control: control.clone(),
            events: events.clone(),
            fps,
        }));

        let job = PlaybackJob {
            scene: self.scene.clone(),
            wake: self.wake.clone(),
            id,
            source,
            looping,
            initial_index,
            stop,
            paused,
            control,
            events,
            generation: self.videos.generation.clone(),
            playback_generation,
            playing,
        };
        std::thread::Builder::new()
            .name("noiria-video".into())
            .spawn(move || playback_loop(job))
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

    pub(crate) fn video_play(
        &mut self,
        id: u32,
        path: &str,
        looping: bool,
    ) -> Result<(), EngineError> {
        log_op!(self, "video_play {id} {path} loop={looping}");
        self.video_play_from(id, path, looping, None, None)
    }

    pub(crate) fn video_play_at(
        &mut self,
        id: u32,
        path: &str,
        looping: bool,
        frame: Option<usize>,
        position_ms: Option<f64>,
    ) -> Result<(), EngineError> {
        log_op!(
            self,
            "video_play_at {id} {path} loop={looping} frame={frame:?} position={position_ms:?}"
        );
        self.video_play_from(id, path, looping, frame, position_ms)
    }

    pub(crate) fn video_stop(&mut self) {
        log_op!(self, "video_stop");
        self.invalidate_current_video();
    }

    pub(crate) fn video_is_playing(&self) -> bool {
        self.videos
            .current
            .as_ref()
            .is_some_and(|c| c.playing.load(Ordering::Relaxed))
    }

    /// [openvideo]（W6）: 動画を開くだけで再生は開始しない。[playvideo] が使う
    /// `prepared` スロットへ格納する（同一レイヤ向けの既存 prepared があれば置き換える）。
    pub(crate) fn video_open(
        &mut self,
        id: u32,
        path: &str,
        looping: bool,
    ) -> Result<(), EngineError> {
        log_op!(self, "video_open {id} {path} loop={looping}");
        let source = self.game_dir.open_video(path).map_err(|e| e.to_string())?;
        {
            let sc = self.scene.lock_unpoisoned();
            if sc.layer(Page::Fore, id).is_none() {
                return Err(format!("レイヤが存在しません: {id}"));
            }
        }
        self.videos.prepared = Some(Box::new(PreparedVideo {
            id,
            source,
            looping,
            initial_index: 0,
            control: Arc::new(Mutex::new(PlaybackControl::default())),
            events: Arc::new(Mutex::new(VideoEvents::default())),
        }));
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
    pub(crate) fn video_prepare(
        &mut self,
        id: u32,
        path: &str,
        looping: bool,
    ) -> Result<(), EngineError> {
        log_op!(self, "video_prepare {id} {path} loop={looping}");
        self.video_open(id, path, looping)?;
        self.prepare_opened_video(id)
    }

    pub(crate) fn video_prepare_opened(&mut self, id: u32) -> Result<(), EngineError> {
        log_op!(self, "video_prepare_opened {id}");
        self.prepare_opened_video(id)
    }

    pub(crate) fn video_seek_frame(&mut self, target: usize) -> Result<(), EngineError> {
        log_op!(self, "video_seek_frame {target}");
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

    pub(crate) fn video_seek_position(&mut self, position_ms: f64) -> Result<(), EngineError> {
        log_op!(self, "video_seek_position {position_ms}");
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

    pub(crate) fn video_set_period_event(&mut self, frame: usize) {
        log_op!(self, "video_set_period_event {frame}");
        let _scene = self.scene.lock_unpoisoned();
        self.videos.for_each_control_and_events(|control, events| {
            control.lock_unpoisoned().period_frame = Some(frame);
            events.lock_unpoisoned().clear(VideoEventKind::Period);
        });
    }

    pub(crate) fn video_cancel_period_event(&mut self) {
        log_op!(self, "video_cancel_period_event");
        let _scene = self.scene.lock_unpoisoned();
        self.videos.for_each_control_and_events(|control, events| {
            control.lock_unpoisoned().period_frame = None;
            events.lock_unpoisoned().clear(VideoEventKind::Period);
        });
    }

    pub(crate) fn video_set_segment_loop(
        &mut self,
        start: usize,
        end: usize,
    ) -> Result<(), EngineError> {
        if start >= end {
            return Err(format!(
                "動画区間は start < end である必要があります: {start}..{end}"
            ));
        }
        log_op!(self, "video_set_segment_loop {start} {end}");
        let _scene = self.scene.lock_unpoisoned();
        self.videos.for_each_control_and_events(|control, events| {
            {
                let mut control = control.lock_unpoisoned();
                control.segment_revision.fetch_add(1, Ordering::SeqCst);
                control.segment_loop = Some((start, end));
            }
            events.lock_unpoisoned().clear(VideoEventKind::SegLoop);
        });
        Ok(())
    }

    pub(crate) fn video_cancel_segment_loop(&mut self) {
        log_op!(self, "video_cancel_segment_loop");
        let _scene = self.scene.lock_unpoisoned();
        self.videos.for_each_control_and_events(|control, events| {
            {
                let mut control = control.lock_unpoisoned();
                control.segment_revision.fetch_add(1, Ordering::SeqCst);
                control.segment_loop = None;
            }
            events.lock_unpoisoned().clear(VideoEventKind::SegLoop);
        });
    }

    pub(crate) fn video_take_event(&mut self, kind: &str) -> bool {
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

    pub(crate) fn video_generation(&self) -> f64 {
        self.videos.generation.load(Ordering::SeqCst) as f64
    }

    /// [playvideo]（storage= 省略時、W6）: `prepared`（openvideo/preparevideo で開いた動画）の
    /// 再生を開始する。準備済みの動画が無い、または別レイヤ向けのものなら失敗させる。
    pub(crate) fn video_play_prepared(&mut self, id: u32) -> Result<(), EngineError> {
        log_op!(self, "video_play_prepared {id}");
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
    pub(crate) fn video_pause(&mut self) {
        log_op!(self, "video_pause");
        let _scene = self.scene.lock_unpoisoned();
        if let Some(current) = self.videos.current.as_ref() {
            current.paused.store(true, Ordering::Relaxed);
        }
    }

    /// [resumevideo]（W6）: 一時停止からの再開。
    pub(crate) fn video_resume(&mut self) {
        log_op!(self, "video_resume");
        let _scene = self.scene.lock_unpoisoned();
        if let Some(current) = self.videos.current.as_ref() {
            current.paused.store(false, Ordering::Relaxed);
        }
    }

    /// [rewindvideo]（W6）: 再生中/一時停止中の動画を先頭へ巻き戻す。
    pub(crate) fn video_rewind(&mut self) {
        log_op!(self, "video_rewind");
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
    pub(crate) fn video_is_paused(&self) -> bool {
        self.videos
            .current
            .as_ref()
            .is_some_and(|c| c.paused.load(Ordering::Relaxed))
    }
}
