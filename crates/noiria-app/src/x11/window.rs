//! `run_windowed` 本体。win32.rs と同型のメッセージループ（アニメ中 16ms /
//! アイドル待ち、InputEvent 送出、settle_finished_tweens・トランジション
//! swap・quake 適用 (`compose_frame` 内部)・last_frame 再描画）を X11 の上で
//! 再現する。`WakeSink::Channel` をそのまま使うため hwnd 相当の起床先スロットは
//! 持たない。
//!
//! win32.rs と同等の機能: リサイズ/最大化可 + レターボックス（X11 コアプロトコルには
//! サーバ側拡縮が無いので `viewport::LetterboxCache`（Win32 と共通の `scale_bilinear`）で CPU 拡縮してから PutImage）、
//! F11 / Alt+Enter で `_NET_WM_STATE_FULLSCREEN`（WM へ root 経由で依頼）、オートリピート
//! 抑止、keyup、閉じる操作の `WindowClose` 転送と 2 秒の強制終了猶予。
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::Select;
use noiria_core::compose::{compose_frame, ComposeBufs};
use noiria_core::pixmap::Pixmap;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Notify;

use super::{conn, keymap, proto};
use crate::bridge::{frame_wait, lock_scene, needs_redraw, InputEvent, SharedScene, UserEvent};
use crate::icon::AppIcon;
use crate::input::{self, CloseGate, KeyAct, KeyEv, KeyTracker, RepeatFilter};
use crate::viewport::{self, LetterboxCache, Viewport};

const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const IDLE_TIMEOUT: Duration = Duration::from_millis(200);
/// KeyRelease をオートリピート対の片割れか確定するまで待つ上限（同一時刻の KeyPress は
/// 同じ書き込みでほぼ同時に届く）。
const REPEAT_PAIR_WAIT: Duration = Duration::from_millis(3);
/// X11 の Mod1（Alt）修飾マスク。
const MOD1_MASK: u16 = 0x8;

/// メインメッセージループの引数一式（win32.rs の `WindowedParams` と同型、
/// `hwnd_slot` は無し。`WakeSink::Channel` がそのまま起床に使えるため）。
pub struct WindowedParams {
    pub title: String,
    pub game_w: u32,
    pub game_h: u32,
    pub scene: SharedScene,
    pub input_tx: UnboundedSender<InputEvent>,
    pub trans_notify: Arc<Notify>,
    pub start: Instant,
    pub user_rx: crossbeam_channel::Receiver<UserEvent>,
    /// ウィンドウ/タスクバーアイコン（ゲーム側 icon.png、無ければエンジン既定）。
    pub icon: AppIcon,
    /// false ならウィンドウを一切可視化しない（テスト用）。
    pub visible: bool,
}

/// セットアップ完了後に必要になる ID 一式。
struct Ids {
    wid: u32,
    gc: u32,
    wm_protocols: u32,
    wm_delete_window: u32,
    net_wm_name: u32,
    utf8_string: u32,
    net_wm_state: u32,
    net_wm_state_fullscreen: u32,
}

/// `_NET_WM_ICON` プロパティのデータ（CARDINAL 配列、ARGB）を組み立てる。
/// 16/32/48 の 3 サイズを連結する（EWMH の仕様どおり、複数サイズを 1 プロパティに
/// 詰めて WM に選ばせる）。各サイズの先頭は `width, height` の 2 CARDINAL、続けて
/// `width*height` 個の ARGB(0xAARRGGBB) CARDINAL。
fn net_wm_icon_data(icon: &AppIcon) -> Vec<u8> {
    let mut out = Vec::new();
    for size in [16u32, 32, 48] {
        let img = icon.image(size);
        out.extend_from_slice(&img.w.to_le_bytes());
        out.extend_from_slice(&img.h.to_le_bytes());
        for px in img.rgba.chunks_exact(4) {
            let (r, g, b, a) = (px[0] as u32, px[1] as u32, px[2] as u32, px[3] as u32);
            let argb = (a << 24) | (r << 16) | (g << 8) | b;
            out.extend_from_slice(&argb.to_le_bytes());
        }
    }
    out
}

fn intern(c: &mut proto::XConn, name: &str) -> Result<u32> {
    let req = proto::req_intern_atom(name, false);
    let reply = c.send_request_and_wait(&req)?;
    proto::parse_intern_atom_reply(&reply)
}

fn set_title(c: &mut proto::XConn, ids: &Ids, title: &str) -> Result<()> {
    c.send_request(&proto::req_change_property(
        ids.wid,
        proto::XA_WM_NAME,
        proto::XA_STRING,
        8,
        title.as_bytes(),
        0,
    ))?;
    c.send_request(&proto::req_change_property(
        ids.wid,
        ids.net_wm_name,
        ids.utf8_string,
        8,
        title.as_bytes(),
        0,
    ))?;
    Ok(())
}

/// 起動時のクライアントサイズ。画面の物理 DPI（mm 寸法から推定。誤差と偽の値を避けるため 108 未満は
/// 96 扱い = 等倍）倍を基本とし、拡大する場合は WM の枠・パネル分として画面の 92% までに収める。
/// 等倍のときは画面いっぱい（枠は WM 任せ）まで許す。
fn initial_size(setup: &conn::Setup, game_w: u32, game_h: u32) -> (u32, u32) {
    let (sw, sh) = (setup.screen_px.0 as u32, setup.screen_px.1 as u32);
    let dpi = if setup.screen_mm.0 > 0 && sw > 0 {
        (sw as f64 * 25.4 / setup.screen_mm.0 as f64).round() as u32
    } else {
        96
    }
    .max(96);
    let dpi = if dpi < 108 { 96 } else { dpi };
    let pct = if dpi > 96 { 92 } else { 100 };
    let (max_w, max_h) = if sw == 0 || sh == 0 {
        (u32::MAX / 2, u32::MAX / 2)
    } else {
        (sw * pct / 100, sh * pct / 100)
    };
    viewport::initial_client_size(game_w, game_h, dpi, max_w, max_h)
}

/// 接続・ウィンドウ ID 一式・keymap・初期クライアントサイズ。
type Connected = (proto::XConn, conn::Setup, Ids, keymap::Keymap, (u32, u32));

/// 接続確立 → ウィンドウ生成 → プロパティ設定 → keymap 取得までを行う。
/// `run_windowed` から呼ばれるほか、テストからも直接呼べる（同一モジュール内）。
fn setup(p: &WindowedParams) -> Result<Connected> {
    let display_str = conn::resolve_display_env()?;
    let spec = conn::parse_display(&display_str)?;
    let mut stream = conn::connect_socket(spec.display)?;
    let (auth_name, auth_data) = conn::find_mit_cookie(spec.display).unwrap_or_default();
    let setup = conn::handshake(&mut stream, &auth_name, &auth_data)?;

    let mut c = proto::XConn::new(stream, setup.resource_id_base, setup.resource_id_mask);

    let wm_protocols = intern(&mut c, "WM_PROTOCOLS")?;
    let wm_delete_window = intern(&mut c, "WM_DELETE_WINDOW")?;
    let net_wm_name = intern(&mut c, "_NET_WM_NAME")?;
    let utf8_string = intern(&mut c, "UTF8_STRING")?;
    let net_wm_icon = intern(&mut c, "_NET_WM_ICON")?;
    let net_wm_state = intern(&mut c, "_NET_WM_STATE")?;
    let net_wm_state_fullscreen = intern(&mut c, "_NET_WM_STATE_FULLSCREEN")?;

    let wid = c.alloc_id();
    let gc = c.alloc_id();
    let ids = Ids {
        wid,
        gc,
        wm_protocols,
        wm_delete_window,
        net_wm_name,
        utf8_string,
        net_wm_state,
        net_wm_state_fullscreen,
    };

    let (init_w, init_h) = initial_size(&setup, p.game_w, p.game_h);
    let event_mask = proto::KEY_PRESS_MASK
        | proto::KEY_RELEASE_MASK
        | proto::BUTTON_PRESS_MASK
        | proto::POINTER_MOTION_MASK
        | proto::EXPOSURE_MASK
        | proto::STRUCTURE_NOTIFY_MASK
        | proto::FOCUS_CHANGE_MASK;
    let create = proto::req_create_window(
        wid,
        setup.root,
        0,
        0,
        init_w.min(u16::MAX as u32) as u16,
        init_h.min(u16::MAX as u32) as u16,
        setup.depth,
        setup.root_visual,
        0,
        event_mask,
    );
    c.send_request(&create)?;

    c.send_request(&proto::req_create_gc(gc, wid))?;

    let hints = proto::wm_size_hints_min((p.game_w / 4).max(160), (p.game_h / 4).max(90));
    c.send_request(&proto::req_change_property(
        wid,
        proto::XA_WM_NORMAL_HINTS,
        proto::XA_WM_SIZE_HINTS,
        32,
        &hints,
        0,
    ))?;

    let protocols_data = wm_delete_window.to_le_bytes();
    c.send_request(&proto::req_change_property(
        wid,
        wm_protocols,
        proto::XA_ATOM,
        32,
        &protocols_data,
        0,
    ))?;

    set_title(&mut c, &ids, &p.title)?;

    let icon_data = net_wm_icon_data(&p.icon);
    c.send_request(&proto::req_change_property(
        wid,
        net_wm_icon,
        proto::XA_CARDINAL,
        32,
        &icon_data,
        0,
    ))?;

    if p.visible {
        c.send_request(&proto::req_map_window(wid))?;
    }

    let count = setup
        .max_keycode
        .saturating_sub(setup.min_keycode)
        .saturating_add(1);
    let req = proto::req_get_keyboard_mapping(setup.min_keycode, count);
    let reply = c.send_request_and_wait(&req)?;
    let km = keymap::parse_reply(setup.min_keycode, &reply)?;

    log::info!(
        "X11: ウィンドウ作成 wid=0x{wid:08x} ({init_w}x{init_h}、ゲーム {}x{})",
        p.game_w,
        p.game_h
    );

    Ok((c, setup, ids, km, (init_w, init_h)))
}

/// ループ中に変化する表示・入力状態（イベントハンドラが共有する）。
struct View {
    game: (u32, u32),
    /// 現在のウィンドウ（クライアント）サイズ。`ConfigureNotify` で更新する。
    win: (u32, u32),
    /// 最後のマウス位置（ウィンドウ座標 px）。
    cursor: (f64, f64),
    /// 直近の合成フレーム（BGRX、ゲーム解像度）。
    last_frame: Vec<u32>,
    /// `last_frame` を書き換えるたびに進む（拡大結果の再利用判定用）。
    frame_gen: u64,
    /// `last_frame` をウィンドウサイズへレターボックス拡縮したもの（等倍時は使わない）。
    letterbox: LetterboxCache,
    keys: KeyTracker,
    repeat: RepeatFilter,
    close_gate: CloseGate,
    start: Instant,
}

impl View {
    fn viewport(&self) -> Viewport {
        viewport::fit(self.win.0, self.win.1, self.game.0, self.game.1)
    }

    fn to_game(&self, px: f64, py: f64) -> (f64, f64) {
        self.viewport().to_game(px, py, self.game.0, self.game.1)
    }

    fn now_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }

    fn send(&self, tx: &UnboundedSender<InputEvent>, kind: &'static str, key: String) {
        let (x, y) = self.to_game(self.cursor.0, self.cursor.1);
        let _ = tx.send(InputEvent {
            kind,
            x,
            y,
            button: None,
            key: Some(key),
            delta: None,
            t: self.now_ms(),
        });
    }

    /// `last_frame` をウィンドウへ present する。ウィンドウサイズがゲーム解像度と同じなら
    /// そのまま、違えばレターボックス付きで拡縮してから PutImage する。
    fn present(&mut self, c: &mut proto::XConn, ids: &Ids, setup: &conn::Setup) -> Result<()> {
        if self.last_frame.is_empty() {
            return Ok(());
        }
        let (w, h) = self.win;
        let (pixels, size): (&[u32], _) = if (w, h) == self.game {
            (&self.last_frame, self.game)
        } else {
            let (px, _) = self.letterbox.get(
                &self.last_frame,
                self.frame_gen,
                self.game.0,
                self.game.1,
                w,
                h,
            );
            (px, (w, h))
        };
        proto::put_image_banded(
            c,
            ids.wid,
            ids.gc,
            pixels,
            size,
            setup.depth,
            setup.max_request_length,
        )
    }
}

/// WM へフルスクリーンの切替（toggle）を依頼する。EWMH: root へ `_NET_WM_STATE`
/// ClientMessage（data = [2(toggle), 属性, 0, 1(通常アプリ), 0]）。WM が無い環境では無効。
fn toggle_fullscreen(c: &mut proto::XConn, ids: &Ids, setup: &conn::Setup) {
    const NET_WM_STATE_TOGGLE: u32 = 2;
    let ev = proto::build_client_message(
        ids.wid,
        ids.net_wm_state,
        [NET_WM_STATE_TOGGLE, ids.net_wm_state_fullscreen, 0, 1, 0],
    );
    let _ = c.send_request(&proto::req_send_event(
        setup.root,
        false,
        proto::ROOT_WM_REQUEST_MASK,
        &ev,
    ));
}

/// 通常起動のメッセージループ。win32.rs と同じ構造（イベントドレイン →
/// UserEvent ドレイン → トランジション/合成/present → 次イベントまで待機）。
pub fn run_windowed(p: WindowedParams) -> Result<()> {
    let (mut c, setup, ids, km, init_size) = setup(&p)?;

    let (x_tx, x_rx) = crossbeam_channel::unbounded::<proto::XEvent>();
    let reader_stream = c
        .stream
        .try_clone()
        .context("X11 ソケットの複製に失敗しました")?;
    let reader_handle = thread::spawn(move || proto::reader_loop(reader_stream, x_tx));

    let mut bufs = ComposeBufs::new(p.game_w, p.game_h);
    let mut frame = Pixmap::new(p.game_w, p.game_h);
    let mut last_drawn_revision = u64::MAX;
    // 動き（文字送り・tween 等）が終わった直後の周期でも 1 回描く。終わった周期では animating が false で
    // revision も変わらないので、描かないと最後のグリフ（文字送りの末尾の 」 など）が次の更新まで出ない。
    let mut was_animating = false;
    let mut last_present = Instant::now() - FRAME_INTERVAL;
    let mut v = View {
        game: (p.game_w, p.game_h),
        win: init_size,
        cursor: (0.0, 0.0),
        last_frame: Vec::new(),
        frame_gen: 0,
        letterbox: LetterboxCache::default(),
        keys: KeyTracker::default(),
        repeat: RepeatFilter::default(),
        close_gate: CloseGate::default(),
        start: p.start,
    };

    'outer: loop {
        // X イベントをドレイン。KeyRelease はオートリピート対の相手が来るか確定するまで
        // 保留されるので、キューが空でも保留があれば少しだけ待つ。
        loop {
            let ev = match x_rx.try_recv() {
                Ok(ev) => ev,
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    if !v.repeat.has_pending() {
                        break;
                    }
                    match x_rx.recv_timeout(REPEAT_PAIR_WAIT) {
                        Ok(ev) => ev,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                            if let Some(KeyAct::Up(kc)) = v.repeat.flush() {
                                handle_key_up(&mut v, &km, &p.input_tx, kc);
                            }
                            break;
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break 'outer,
                    }
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => break 'outer,
            };
            match ev {
                proto::XEvent::KeyPress {
                    keycode,
                    state,
                    time,
                } => {
                    for act in v.repeat.feed(KeyEv::Press { keycode, time }) {
                        match act {
                            KeyAct::Up(kc) => handle_key_up(&mut v, &km, &p.input_tx, kc),
                            KeyAct::Down(kc) => {
                                handle_key_down(&mut v, &km, &p.input_tx, &mut c, &ids, &setup, kc, state)
                            }
                        }
                    }
                }
                proto::XEvent::KeyRelease { keycode, time } => {
                    for act in v.repeat.feed(KeyEv::Release { keycode, time }) {
                        if let KeyAct::Up(kc) = act {
                            handle_key_up(&mut v, &km, &p.input_tx, kc);
                        }
                    }
                }
                proto::XEvent::FocusOut => {
                    // フォーカスを失うとキーアップが届かない。押しっぱなし扱いを残さない。
                    for k in v.keys.release_all() {
                        v.send(&p.input_tx, "keyup", k);
                    }
                }
                proto::XEvent::ButtonPress {
                    button,
                    event_x,
                    event_y,
                } => {
                    v.cursor = (event_x as f64, event_y as f64);
                    let (x, y) = v.to_game(v.cursor.0, v.cursor.1);
                    match button {
                        1..=3 => {
                            let b = match button {
                                1 => "left",
                                2 => "middle",
                                _ => "right",
                            };
                            let _ = p.input_tx.send(InputEvent {
                                kind: "click",
                                x,
                                y,
                                button: Some(b),
                                key: None,
                                delta: None,
                                t: v.now_ms(),
                            });
                        }
                        4 | 5 => {
                            let delta = if button == 4 { 1.0 } else { -1.0 };
                            let _ = p.input_tx.send(InputEvent {
                                kind: "wheel",
                                x,
                                y,
                                button: None,
                                key: None,
                                delta: Some(delta),
                                t: v.now_ms(),
                            });
                        }
                        _ => {}
                    }
                }
                proto::XEvent::MotionNotify { event_x, event_y } => {
                    v.cursor = (event_x as f64, event_y as f64);
                    let (x, y) = v.to_game(v.cursor.0, v.cursor.1);
                    let _ = p.input_tx.send(InputEvent {
                        kind: "move",
                        x,
                        y,
                        button: None,
                        key: None,
                        delta: None,
                        t: v.now_ms(),
                    });
                }
                proto::XEvent::ConfigureNotify { width, height } => {
                    let size = ((width as u32).max(1), (height as u32).max(1));
                    if size != v.win {
                        v.win = size;
                        let _ = v.present(&mut c, &ids, &setup);
                    }
                }
                proto::XEvent::Expose => {
                    let _ = v.present(&mut c, &ids, &setup);
                }
                proto::XEvent::ClientMessage {
                    message_type,
                    data0,
                } => {
                    if message_type == ids.wm_protocols && data0 == ids.wm_delete_window {
                        // 即終了せず、スクリプトへ WindowClose を送って猶予内の終了要求を待つ。
                        let alive = !p.input_tx.is_closed();
                        if v.close_gate.request(Instant::now(), alive) {
                            break 'outer;
                        }
                        v.send(&p.input_tx, "key", input::KEY_WINDOW_CLOSE.to_string());
                    }
                }
                proto::XEvent::Other => {}
            }
        }

        // UserEvent をドレイン。
        let mut quit = false;
        while let Ok(ev) = p.user_rx.try_recv() {
            match ev {
                UserEvent::Redraw => {}
                UserEvent::SetTitle(title) => {
                    let _ = set_title(&mut c, &ids, &title);
                }
                UserEvent::Quit => quit = true,
            }
        }
        if quit {
            break 'outer;
        }
        let close_remaining = v.close_gate.remaining(Instant::now());
        if v.close_gate.expired(Instant::now()) {
            log::warn!("閉じる要求にスクリプトが応答しなかったため強制終了します");
            break 'outer;
        }

        // トランジション完了のステート遷移 + 合成 + PutImage。合成は最短でも FRAME_INTERVAL 間隔、シーンのロックは
        // 状態の更新とスナップショットの複製の間だけ握る（win32 側と同じ。握ったままだとスクリプトが合成のたびに待たされる）。
        let now = Instant::now();
        let animating;
        let mut frame_due_in = None;
        let snapshot = {
            let mut scene = lock_scene(&p.scene);
            if scene
                .transition
                .as_ref()
                .is_some_and(|t| t.is_finished(now))
            {
                scene.swap_after_transition();
                p.trans_notify.notify_waiters();
            }
            scene.settle_finished_tweens(now);
            animating = scene.is_animating(now);
            let revision = scene.revision;
            if needs_redraw(revision, last_drawn_revision, animating, was_animating) {
                let due = last_present + FRAME_INTERVAL;
                if now >= due {
                    last_drawn_revision = revision;
                    Some(scene.clone())
                } else {
                    frame_due_in = Some(due - now);
                    None
                }
            } else {
                None
            }
        };
        if let Some(scene) = snapshot {
            last_present = now;
            compose_frame(&scene, now, &mut bufs, &mut frame);
            v.last_frame
                .resize((frame.width as usize) * (frame.height as usize), 0);
            frame.to_bgrx_u32(&mut v.last_frame);
            v.frame_gen += 1;
            v.present(&mut c, &ids, &setup)?;
            was_animating = animating;
        } else if frame_due_in.is_none() {
            was_animating = animating;
        }

        let timeout = frame_wait(
            animating,
            FRAME_INTERVAL,
            last_present.elapsed(),
            frame_due_in,
            close_remaining,
            IDLE_TIMEOUT,
        );
        let mut sel = Select::new();
        let _x_idx = sel.recv(&x_rx);
        let _user_idx = sel.recv(&p.user_rx);
        let _ = sel.ready_timeout(timeout);
    }

    let _ = c.stream.shutdown(std::net::Shutdown::Both);
    let _ = reader_handle.join();
    Ok(())
}

/// 初回押下（オートリピートは `RepeatFilter` が除去済み）。F11 / Alt+Enter はホストが消費する。
#[allow(clippy::too_many_arguments)]
fn handle_key_down(
    v: &mut View,
    km: &keymap::Keymap,
    tx: &UnboundedSender<InputEvent>,
    c: &mut proto::XConn,
    ids: &Ids,
    setup: &conn::Setup,
    keycode: u8,
    state: u16,
) {
    let col = if state & 0x1 != 0 { 1 } else { 0 };
    let keysym = km.keysym(keycode, col);
    let base_sym = km.keysym(keycode, 0);
    if keysym == keymap::KS_F11 || (base_sym == keymap::KS_RETURN && state & MOD1_MASK != 0) {
        toggle_fullscreen(c, ids, setup);
        return;
    }
    if let Some(key) = keymap::to_input_key(keysym) {
        if input::is_keyup_tracked(&key) {
            v.keys.press(&key);
        }
        v.send(tx, "key", key);
    }
}

fn handle_key_up(v: &mut View, km: &keymap::Keymap, tx: &UnboundedSender<InputEvent>, keycode: u8) {
    if let Some(key) = keymap::to_input_key(km.keysym(keycode, 0)) {
        if input::is_keyup_tracked(&key) && v.keys.release(&key) {
            v.send(tx, "keyup", key);
        }
    }
}

#[cfg(test)]
mod tests;
