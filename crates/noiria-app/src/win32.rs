//! Win32 API 直叩きのウィンドウ/描画/入力。winit + softbuffer の置き換え。
//!
//! - ウィンドウ生成: `RegisterClassW` + `CreateWindowExW`。リサイズ/最大化可
//!   (`WS_OVERLAPPEDWINDOW`)。初期クライアントサイズはカーソルのあるモニタの DPI 倍率
//!   （非整数倍も可）で、作業領域に収まる最大まで拡大し、作業領域の中央へ置く
//!   （`viewport::initial_client_size`）。`WM_DPICHANGED` に追随する。
//! - 描画: 合成済みフレーム (`Pixmap::to_bgrx_u32`) を、アスペクト比を保ったレターボックス
//!   (`viewport::fit`) で `StretchDIBits` 提示する（帯は `PatBlt` で黒。拡縮時は HALFTONE）。
//!   マウス座標は同じ `Viewport` で論理座標へ逆変換してスクリプトへ渡す。
//! - フルスクリーン: F11 / Alt+Enter でボーダーレス（`WS_POPUP` + モニタ矩形）切替。
//! - メッセージループ: `MsgWaitForMultipleObjects`（アニメ中は約16ms タイムアウト、
//!   アイドル時は無限待機）+ `PeekMessage` でメッセージをドレインする。
//! - 入力: `WM_MOUSEMOVE` / `WM_{L,R,M}BUTTONDOWN` / `WM_MOUSEWHEEL` / `WM_KEYDOWN`・`WM_KEYUP`
//!   / `WM_CHAR` を `InputEvent` に変換してスクリプトスレッドへ送出する。キーのオート
//!   リピートは送らない（初回押下のみ）。閉じる操作は即 Destroy せず `key:"WindowClose"` を
//!   スクリプトへ送り、2 秒以内に終了要求が無ければ強制終了する（`input::CloseGate`）。
//!
//! `visible: false` を渡すと `WS_VISIBLE` を付けず `ShowWindow` も呼ばない
//! （不可視ウィンドウテスト用）。

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use noiria_core::compose::{compose_frame, ComposeBufs};
use noiria_core::pixmap::Pixmap;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Notify;

use windows::core::HSTRING;
use windows::Win32::Foundation::{HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, EndPaint, GetDC, SelectObject, GetMonitorInfoW, InvalidateRect, MonitorFromPoint,
    MonitorFromWindow, PatBlt, ReleaseDC, ScreenToClient, SetBrushOrgEx, SetStretchBltMode,
    StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BLACKNESS, COLORONCOLOR, DIB_RGB_COLORS, HALFTONE,
    HDC, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForMonitor, GetDpiForWindow, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_F11, VK_F5, VK_F9, VK_NEXT, VK_PRIOR,
    VK_RETURN, VK_SHIFT, VK_SPACE, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow, GetClientRect,
    DispatchMessageW, GetCursorPos, GetWindowLongPtrW, GetWindowPlacement, LoadCursorW,
    KillTimer, MsgWaitForMultipleObjects, PeekMessageW, PostMessageW, PostQuitMessage, RegisterClassW,
    SendMessageW, SetTimer, SetWindowLongPtrW, SetWindowPlacement, SetWindowPos, SetWindowTextW, ShowWindow,
    TranslateMessage, CREATESTRUCTW, GWLP_USERDATA, GWL_STYLE, HICON, HWND_TOP, IDC_ARROW,
    IMAGE_FLAGS, MINMAXINFO, MSG, PM_REMOVE, QS_ALLINPUT, SIZE_MINIMIZED, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SW_SHOW, WINDOWPLACEMENT,
    WINDOW_EX_STYLE, WM_APP, WM_CHAR, WM_CLOSE, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND,
    WM_ENTERMENULOOP, WM_ENTERSIZEMOVE, WM_EXITMENULOOP, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_MBUTTONDOWN,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_PAINT, WM_QUIT, WM_RBUTTONDOWN, WM_SETICON,
    WM_SIZE, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_POPUP,
    WS_VISIBLE,
};

use crate::bridge::{frame_wait, lock_scene, needs_redraw, InputEvent, SharedScene, UserEvent};
use crate::icon::AppIcon;
use crate::input::{self, CloseGate, KeyTracker};
use crate::viewport::{self, ScaleCache, Viewport};

const FRAME_INTERVAL: Duration = Duration::from_millis(16);
/// タイトルバーのドラッグ・リサイズ・システムメニューの間（OS のモーダルループ中。外側のループは止まる）に
/// フレーム処理を駆動する `WM_TIMER` の ID。
const MODAL_TIMER_ID: usize = 1;
const WHEEL_DELTA: f64 = 120.0;
const CLASS_NAME: &str = "NoiriaWindowClass\0";
/// `WM_KEYDOWN` の lParam bit 30（直前もキーが押されていた = オートリピート）。
const KEY_PREV_STATE: isize = 1 << 30;

/// メインメッセージループの引数一式。
pub struct WindowedParams {
    pub title: String,
    pub game_w: u32,
    pub game_h: u32,
    pub scene: SharedScene,
    pub input_tx: UnboundedSender<InputEvent>,
    pub trans_notify: Arc<Notify>,
    pub start: Instant,
    pub user_rx: crossbeam_channel::Receiver<UserEvent>,
    /// ウィンドウ生成後に hwnd (isize 化) を格納する。`WakeSink::Win32` が
    /// スクリプトスレッドからの `PostMessageW` 起床先として参照する。
    pub hwnd_slot: Arc<AtomicIsize>,
    /// `WakeSink::Win32` と同じ「起こし投函済み」フラグ（窓ごと）。
    pub wake_posted: Arc<AtomicBool>,
    /// ウィンドウ/タスクバーアイコン（ゲーム側 icon.png、無ければエンジン既定）。
    pub icon: AppIcon,
    /// false ならウィンドウを一切可視化しない（テスト用）。
    pub visible: bool,
}

/// フルスクリーンへ入る前のウィンドウ状態（復帰用）。
struct SavedWindow {
    style: u32,
    placement: WINDOWPLACEMENT,
}

/// wndproc からアクセスする共有状態。`GWLP_USERDATA` 経由でポインタを渡す。
/// メッセージループとウィンドウプロシージャは常に同一スレッドで動くので
/// 追加の同期無しでも安全（Win32 のメッセージディスパッチはリエントラントではない）。
///
/// ただし `SetWindowPos` 等は WM_SIZE などを同期的に wndproc へ送り返す（リエントラント）。
/// RefCell を借りたまま Win32 を呼ぶと二重借用 panic になるので、借用は短く保つこと。
struct WindowShared {
    input_tx: UnboundedSender<InputEvent>,
    game_w: u32,
    game_h: u32,
    client_size: Cell<(u32, u32)>,
    /// 最後のマウス位置（クライアント座標 px）。
    cursor: Cell<(f64, f64)>,
    start: Instant,
    /// 直近に present したフレーム（BGRX u32 列）。WM_PAINT での再描画に使う。
    last_frame: RefCell<Vec<u32>>,
    last_frame_size: Cell<(u32, u32)>,
    /// 最後のクリック/キー入力を受けた時刻（`NOIRIA_PROF` の `since_input` 用）。
    last_input: Cell<Option<Instant>>,
    /// `last_frame` を書き換えるたびに進む（拡大済みバッファの有効判定用）。
    frame_gen: Cell<u64>,
    /// 表示サイズへ拡大した `last_frame` と、それが対応する（`frame_gen`, 幅, 高さ）。
    scaled: RefCell<ScaleCache>,
    /// メッセージループの外側（`run_windowed`）とモーダルループ中の `WM_TIMER` の両方から回すフレーム処理の状態。
    frame_loop: RefCell<FrameLoop>,
    wake_posted: Arc<AtomicBool>,
    hwnd_slot: Arc<AtomicIsize>,
    close_gate: RefCell<CloseGate>,
    keys: RefCell<KeyTracker>,
    fullscreen: Cell<bool>,
    saved: RefCell<Option<SavedWindow>>,
}

impl WindowShared {
    /// 表示用のバッファ（等倍ならそのまま、そうでなければ双一次で表示サイズへ拡大済み）を `f` へ渡す。
    /// 拡大は `StretchDIBits` の HALFTONE に任せない（1920x1080 で 1 フレーム約 15ms。`noiria_core::scale` 参照）。
    /// 同じフレームの再表示（WM_PAINT）では拡大し直さない。戻り値は拡大に使った時間。
    fn with_display<R>(&self, f: impl FnOnce(&[u32], u32, u32) -> R) -> (R, Duration) {
        let vp = self.viewport();
        let (fw, fh) = self.last_frame_size.get();
        let frame = self.last_frame.borrow();
        if fw == 0 || fh == 0 || (vp.w == fw && vp.h == fh) || vp.w == 0 || vp.h == 0 {
            return (f(&frame, fw, fh), Duration::ZERO);
        }
        let t0 = Instant::now();
        let mut cache = self.scaled.borrow_mut();
        let scaled = cache.get(&frame, self.frame_gen.get(), fw, fh, vp.w, vp.h);
        let spent = t0.elapsed();
        (f(scaled, vp.w, vp.h), spent)
    }

    fn now_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }

    fn viewport(&self) -> Viewport {
        let (cw, ch) = self.client_size.get();
        viewport::fit(cw, ch, self.game_w, self.game_h)
    }

    /// クライアント座標 → ゲーム座標（レターボックス・表示倍率の逆変換）。
    fn to_game_coords(&self, px: f64, py: f64) -> (f64, f64) {
        self.viewport().to_game(px, py, self.game_w, self.game_h)
    }

    fn send_input(&self, ev: InputEvent) {
        let _ = self.input_tx.send(ev);
    }

    fn send_move_or_click(
        &self,
        kind: &'static str,
        button: Option<&'static str>,
        px: f64,
        py: f64,
    ) {
        let (x, y) = self.to_game_coords(px, py);
        self.send_input(InputEvent {
            kind,
            x,
            y,
            button,
            key: None,
            delta: None,
            t: self.now_ms(),
        });
    }

    fn send_key_event(&self, kind: &'static str, key: String) {
        let (x, y) = self.to_game_coords(self.cursor.get().0, self.cursor.get().1);
        self.send_input(InputEvent {
            kind,
            x,
            y,
            button: None,
            key: Some(key),
            delta: None,
            t: self.now_ms(),
        });
    }

    fn send_key(&self, key: String) {
        self.send_key_event("key", key);
    }

    fn send_keyup(&self, key: String) {
        self.send_key_event("keyup", key);
    }
}

fn loword(v: u32) -> i16 {
    (v & 0xFFFF) as u16 as i16
}
fn hiword(v: u32) -> i16 {
    ((v >> 16) & 0xFFFF) as u16 as i16
}

/// キーのオートリピートか（lParam bit 30 = 直前の状態が押下）。
fn is_autorepeat(lparam: isize) -> bool {
    lparam & KEY_PREV_STATE != 0
}

/// 仮想キー → スクリプトへ渡す key 名（特殊キーのみ。印字文字は WM_CHAR 経由）。
fn vk_to_key(vk: VIRTUAL_KEY) -> Option<&'static str> {
    match vk {
        VK_RETURN => Some("Enter"),
        VK_ESCAPE => Some("Escape"),
        VK_CONTROL => Some("Control"),
        VK_SHIFT => Some("Shift"),
        VK_SPACE => Some(" "),
        VK_UP => Some("ArrowUp"),
        VK_DOWN => Some("ArrowDown"),
        VK_PRIOR => Some("PageUp"),
        VK_NEXT => Some("PageDown"),
        VK_F5 => Some("F5"),
        VK_F9 => Some("F9"),
        _ => None,
    }
}

/// スクリプトスレッドから `WakeSink::Win32` 経由でメインループを起こすための
/// `PostMessageW(hwnd, WM_APP, ...)`。hwnd が未設定 (0) なら何もしない。
/// `posted` は窓ごとの「起こしのメッセージが既に投函済み（まだ描画ループが取り込んでいない）か」。ops 1 つごとに
/// `PostMessageW` すると数百 op のメニュー構築で数百回メッセージを投げ、描画ループがそのたびに起きて合成していた。
/// プロセス全体の static にしないのは、別窓（並列テストなど）の起こしを互いに抑止してしまうため。
pub(crate) fn post_wake(hwnd_raw: isize, posted: &AtomicBool) {
    if hwnd_raw == 0 {
        return;
    }
    if posted.swap(true, Ordering::SeqCst) {
        return;
    }
    let hwnd = HWND(hwnd_raw as *mut c_void);
    // SAFETY: `PostMessageW` はどのスレッドからでも呼べ、`hwnd` が破棄済み・無効でもエラーを返すだけで副作用はない
    // （`hwnd_raw` は窓の破棄時に 0 へ戻され、遅れた起こしはそこで止まる）。
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_APP, WPARAM(0), LPARAM(0));
    }
}

/// GWLP_USERDATA から `WindowShared` への参照を取り出す。ウィンドウ生成前
/// (WM_NCCREATE 未処理) は `None`。
///
/// # Safety
/// `hwnd` は `run_windowed` が `CreateWindowExW` に `Box<WindowShared>` のポインタ（`lpCreateParams`）を渡して作った
/// 窓でなければならず、呼び出しは窓スレッドからで、`run_windowed` が `WM_QUIT` 後に Box を回収するより前であること
/// （その間 `GWLP_USERDATA` は null か、生きている `WindowShared` を指す）。返す参照は共有参照のみで、
/// 内部可変性は `Cell`/`RefCell`（同一スレッド専用）に限られる。
unsafe fn shared_of<'a>(hwnd: HWND) -> Option<&'a WindowShared> {
    // SAFETY: 上記の前提（USERDATA は null か有効な `WindowShared` へのポインタ）による。
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WindowShared).as_ref() }
}

/// フレームバッファ (BGRX u32 列, `fw`×`fh`) をクライアント領域 (`cw`×`ch`) へ
/// レターボックス付きで `StretchDIBits` 拡縮描画する。
fn blit_dc(hdc: HDC, buf: &[u32], fw: u32, fh: u32, cw: u32, ch: u32, vp: Viewport) {
    if buf.is_empty() || fw == 0 || fh == 0 || cw == 0 || ch == 0 {
        return;
    }
    // `StretchDIBits` は `fw`×`fh` 画素ぶんを読む。足りないバッファを渡すと範囲外を読むので描かない。
    if buf.len() < fw as usize * fh as usize {
        log::error!("blit_dc: バッファが足りません ({} < {fw}x{fh})", buf.len());
        return;
    }
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: fw as i32,
            // トップダウン DIB にするため負値。
            biHeight: -(fh as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0, // BI_RGB
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: `hdc` は呼び出し側（`GetDC`/`BeginPaint`/メモリ DC）が有効に保つ。`bmi` はスタック上で呼び出しの間生きており、
    // `buf` は上の検査で `fw`×`fh` 画素以上あるので `StretchDIBits` の読み出しは範囲内。
    unsafe {
        // 黒帯（WM_ERASEBKGND は無効化しているので自前で塗る。ゲーム領域には触れない）。
        for (x, y, w, h) in viewport::letterbox_bands(vp, cw, ch) {
            let _ = PatBlt(hdc, x, y, w, h, BLACKNESS);
        }
        if vp.w == fw && vp.h == fh {
            SetStretchBltMode(hdc, COLORONCOLOR);
        } else {
            SetStretchBltMode(hdc, HALFTONE);
            let _ = SetBrushOrgEx(hdc, 0, 0, None);
        }
        StretchDIBits(
            hdc,
            vp.x,
            vp.y,
            vp.w as i32,
            vp.h as i32,
            0,
            0,
            fw as i32,
            fh as i32,
            Some(buf.as_ptr() as *const c_void),
            &bmi,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}

/// 現在の合成フレームをウィンドウへ present する。合成結果は WM_PAINT による
/// 再描画のために `WindowShared::last_frame` へも保存する。
fn present(hwnd: HWND, shared: &WindowShared, frame: &Pixmap) {
    present_timed(hwnd, shared, frame);
}

/// `present` と同じで、（BGRX 変換, 拡大, 転送, 転送のメモリ DC 版）の所要時間を返す（`NOIRIA_PROF` 用）。
fn present_timed(hwnd: HWND, shared: &WindowShared, frame: &Pixmap) -> (Duration, Duration, Duration, Duration) {
    let t0 = Instant::now();
    {
        let mut buf = shared.last_frame.borrow_mut();
        buf.resize((frame.width as usize) * (frame.height as usize), 0);
        frame.to_bgrx_u32(&mut buf);
    }
    shared.last_frame_size.set((frame.width, frame.height));
    shared.frame_gen.set(shared.frame_gen.get() + 1);
    let convert = t0.elapsed();
    let t1 = Instant::now();
    let (cw, ch) = shared.client_size.get();
    let mut scale = Duration::ZERO;
    // SAFETY: `hwnd` は描画ループが生成した生きている窓。`GetDC` の戻りは `is_invalid` で確かめてから使い、
    // 必ず同じ窓へ `ReleaseDC` で返す。
    let hdc = unsafe { GetDC(Some(hwnd)) };
    if !hdc.is_invalid() {
        let vp = shared.viewport();
        let (_, spent) = shared.with_display(|buf, w, h| blit_dc(hdc, buf, w, h, cw, ch, vp));
        scale = spent;
        // SAFETY: 直前に同じ `hwnd` で取得した有効な DC を解放する（以後使わない）。
        unsafe {
            ReleaseDC(Some(hwnd), hdc);
        }
    }
    let blit = t1.elapsed().saturating_sub(scale);
    let mut mem = Duration::ZERO;
    if crate::prof::deep() {
        let t2 = Instant::now();
        let (cw, ch) = shared.client_size.get();
        // SAFETY: 計測専用の経路。`GetDC` で得た画面 DC・互換 DC・ビットマップは、選択を元へ戻したうえで
        // この塊の中で全て解放し、`blit_dc` へ渡す DC は塊の中でだけ使う。
        unsafe {
            let screen = GetDC(Some(hwnd));
            let dc = CreateCompatibleDC(Some(screen));
            let bmp = CreateCompatibleBitmap(screen, cw as i32, ch as i32);
            let old = SelectObject(dc, bmp.into());
            let vp = shared.viewport();
            shared.with_display(|buf, w, h| blit_dc(dc, buf, w, h, cw, ch, vp));
            SelectObject(dc, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(dc);
            ReleaseDC(Some(hwnd), screen);
        }
        mem = t2.elapsed();
    }
    (convert, scale, blit, mem)
}

/// ボーダーレスフルスクリーン ⇔ 通常ウィンドウの切替（Raymond Chen 式: `WS_OVERLAPPEDWINDOW`
/// を外して `WS_POPUP` + モニタ矩形、復帰は保存したスタイルと `WINDOWPLACEMENT`）。
/// 不可視ウィンドウでも `WS_VISIBLE` の有無は保たれる（切替で可視化しない）。
fn toggle_fullscreen(hwnd: HWND, shared: &WindowShared) {
    toggle_fullscreen_inner(hwnd, shared);
    // WM_SIZE の到着順に依存せず、切替直後から後続の入力が新しいサイズで逆変換されるようにする。
    let mut rc = RECT::default();
    // SAFETY: `rc` は呼び出しの間生きているスタック上の RECT。`hwnd` が無効でも `GetClientRect` は Err を返すだけ。
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_ok() && rc.right > rc.left && rc.bottom > rc.top {
        shared
            .client_size
            .set(((rc.right - rc.left) as u32, (rc.bottom - rc.top) as u32));
        // SAFETY: 全域無効化の要求だけ（ポインタ引数なし）。
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    }
}

fn toggle_fullscreen_inner(hwnd: HWND, shared: &WindowShared) {
    // 借用は短く保つ（SetWindowPos 等は WM_SIZE を同期的に wndproc へ送り返す）。
    let prev = shared.saved.borrow_mut().take();
    if let Some(s) = prev {
        shared.fullscreen.set(false);
        // SAFETY: 以下の Win32 呼び出しは、窓スレッドから自分の窓 `hwnd` へ行う。渡すポインタは
        // スタック/`s` 上の値（`WINDOWPLACEMENT`）への参照で、呼び出しの間生きている。
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_STYLE, s.style as isize);
            if s.style & WS_VISIBLE.0 != 0 {
                let _ = SetWindowPlacement(hwnd, &s.placement);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
                );
            } else {
                // 不可視ウィンドウに SetWindowPlacement を使うと表示されてしまう（テスト専用経路）。
                let rc = s.placement.rcNormalPosition;
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    rc.left,
                    rc.top,
                    rc.right - rc.left,
                    rc.bottom - rc.top,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
        }
        return;
    }
    // SAFETY: 窓スレッドから自分の窓へのスタイル取得。
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: `placement` は `length` を設定済みのスタック上の構造体。
    if unsafe { GetWindowPlacement(hwnd, &mut placement) }.is_err() {
        return;
    }
    // SAFETY: `MonitorFromWindow` は値を返すだけ。
    let mon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `mi` は `cbSize` を設定済みのスタック上の構造体。
    if !unsafe { GetMonitorInfoW(mon, &mut mi) }.as_bool() {
        return;
    }
    *shared.saved.borrow_mut() = Some(SavedWindow { style, placement });
    shared.fullscreen.set(true);
    let rc = mi.rcMonitor;
    // SAFETY: 窓スレッドから自分の窓へのスタイル変更と移動（ポインタ引数なし）。
    unsafe {
        SetWindowLongPtrW(
            hwnd,
            GWL_STYLE,
            ((style & !WS_OVERLAPPEDWINDOW.0) | WS_POPUP.0) as isize,
        );
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            rc.left,
            rc.top,
            rc.right - rc.left,
            rc.bottom - rc.top,
            SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
        );
    }
}

/// 閉じる操作: 即 Destroy せず `WindowClose` をスクリプトへ送り、猶予（`CLOSE_GRACE`）を
/// 開始する。スクリプトが既に死んでいれば（入力受信口が閉じている）その場で破棄する。
fn request_close(hwnd: HWND, shared: &WindowShared) {
    let script_alive = !shared.input_tx.is_closed();
    let destroy_now = shared
        .close_gate
        .borrow_mut()
        .request(Instant::now(), script_alive);
    if destroy_now {
        // SAFETY: 窓スレッドから自分の窓を破棄する（WM_DESTROY で PostQuitMessage する）。
        let _ = unsafe { DestroyWindow(hwnd) };
    } else {
        shared.send_key(input::KEY_WINDOW_CLOSE.to_string());
    }
}

/// `wndproc` からの `shared_of`。
fn shared_in_wndproc<'a>(hwnd: HWND) -> Option<&'a WindowShared> {
    // SAFETY: 呼び出し元は `wndproc`（窓スレッドで、`run_windowed` が `WindowShared` を生かしている間だけ呼ばれる。
    // このクラスの窓は `run_windowed` だけが作り、`lpCreateParams` に `WindowShared` のポインタを渡す）。
    unsafe { shared_of(hwnd) }
}

/// 窓プロシージャ。`RegisterClassW` へ渡すために `unsafe extern "system"` だが、本体は `wndproc_impl`（安全な関数）で、
/// 呼び出し側（OS）に課す条件は「有効な窓ハンドルとそのメッセージ」だけ。
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    wndproc_impl(hwnd, msg, wparam, lparam)
}

fn wndproc_impl(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: この関数内の `DefWindowProcW` 呼び出し全般。OS から渡された引数をそのまま既定処理へ返すだけ。
    let default = || unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    match msg {
        WM_NCCREATE => {
            // SAFETY: `WM_NCCREATE` の lParam は `CREATESTRUCTW` へのポインタ（OS が保証。呼び出しの間有効）。
            // `lpCreateParams` は `run_windowed` が渡した `Box<WindowShared>` のポインタで、`WM_QUIT` 後の回収まで有効。
            unsafe {
                let cs = &*(lparam.0 as *const CREATESTRUCTW);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            }
            default()
        }
        WM_GETMINMAXINFO => {
            // 生成中（WM_NCCREATE より前）にも送られる。その間は既定のまま。
            if let Some(shared) = shared_in_wndproc(hwnd) {
                let (w, h) = (
                    (shared.game_w / 4).max(160),
                    (shared.game_h / 4).max(90),
                );
                let mut rc = RECT {
                    left: 0,
                    top: 0,
                    right: w as i32,
                    bottom: h as i32,
                };
                // SAFETY: `WM_GETMINMAXINFO` の lParam は `MINMAXINFO` へのポインタ（OS が保証。処理の間有効で書き込み可）。
                // `rc` はスタック上の RECT。窓スレッドから自分の窓のスタイル・DPI を読むだけ。
                unsafe {
                    let mmi = &mut *(lparam.0 as *mut MINMAXINFO);
                    let style = windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
                        GetWindowLongPtrW(hwnd, GWL_STYLE) as u32,
                    );
                    let _ = AdjustWindowRectExForDpi(
                        &mut rc,
                        style,
                        false,
                        WINDOW_EX_STYLE(0),
                        GetDpiForWindow(hwnd),
                    );
                    mmi.ptMinTrackSize = POINT {
                        x: rc.right - rc.left,
                        y: rc.bottom - rc.top,
                    };
                }
                return LRESULT(0);
            }
            default()
        }
        WM_SIZE => {
            if let Some(shared) = shared_in_wndproc(hwnd) {
                if wparam.0 as u32 != SIZE_MINIMIZED {
                    let raw = lparam.0 as u32;
                    let w = (raw & 0xFFFF).max(1);
                    let h = ((raw >> 16) & 0xFFFF).max(1);
                    shared.client_size.set((w, h));
                    // SAFETY: 全域無効化の要求だけ（ポインタ引数なし）。
                    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                }
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // 移動先モニタの DPI に合わせた推奨矩形へ。フルスクリーン中は何もしない。
            if let Some(shared) = shared_in_wndproc(hwnd) {
                if !shared.fullscreen.get() {
                    // SAFETY: `WM_DPICHANGED` の lParam は推奨矩形 `RECT` へのポインタ（OS が保証。処理の間有効）。
                    unsafe {
                        let rc = &*(lparam.0 as *const RECT);
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            rc.left,
                            rc.top,
                            rc.right - rc.left,
                            rc.bottom - rc.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                }
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        // OS のモーダルループ（タイトルバーのドラッグ・リサイズ・システムメニュー）の間は、外側のメッセージループが
        // 止まる。その間も `WM_TIMER` でフレーム処理を回し、トランジションの完了・終了要求・描き直しを止めない。
        WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
            // SAFETY: 自分の窓へのタイマー登録（コールバックなし）。
            unsafe {
                SetTimer(Some(hwnd), MODAL_TIMER_ID, FRAME_INTERVAL.as_millis() as u32, None);
            }
            default()
        }
        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            // SAFETY: `SetTimer` で登録した自分のタイマーの解除（未登録でも Err を返すだけ）。
            let _ = unsafe { KillTimer(Some(hwnd), MODAL_TIMER_ID) };
            default()
        }
        WM_TIMER if wparam.0 == MODAL_TIMER_ID => {
            if let Some(shared) = shared_in_wndproc(hwnd) {
                tick(hwnd, shared);
            }
            LRESULT(0)
        }
        #[cfg(test)]
        tests::WM_TEST_MODAL_PUMP => {
            tests::modal_pump(hwnd, wparam.0 as u64);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if let Some(shared) = shared_in_wndproc(hwnd) {
                let raw = lparam.0 as u32;
                let (px, py) = (loword(raw) as f64, hiword(raw) as f64);
                shared.cursor.set((px, py));
                shared.send_move_or_click("move", None, px, py);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            if let Some(shared) = shared_in_wndproc(hwnd) {
                let raw = lparam.0 as u32;
                let (px, py) = (loword(raw) as f64, hiword(raw) as f64);
                shared.cursor.set((px, py));
                shared.last_input.set(Some(Instant::now()));
                let button = match msg {
                    WM_LBUTTONDOWN => "left",
                    WM_RBUTTONDOWN => "right",
                    _ => "middle",
                };
                shared.send_move_or_click("click", Some(button), px, py);
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            if let Some(shared) = shared_in_wndproc(hwnd) {
                let wraw = wparam.0 as u32;
                let delta = hiword(wraw) as f64 / WHEEL_DELTA;
                let lraw = lparam.0 as u32;
                let mut pt = POINT {
                    x: loword(lraw) as i32,
                    y: hiword(lraw) as i32,
                };
                // WM_MOUSEWHEEL の lParam はスクリーン座標なのでクライアント座標へ変換する。
                // SAFETY: `pt` はスタック上の POINT。
                let _ = unsafe { ScreenToClient(hwnd, &mut pt) };
                shared.cursor.set((pt.x as f64, pt.y as f64));
                let (x, y) = shared.to_game_coords(pt.x as f64, pt.y as f64);
                shared.send_input(InputEvent {
                    kind: "wheel",
                    x,
                    y,
                    button: None,
                    key: None,
                    delta: Some(delta),
                    t: shared.now_ms(),
                });
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if let Some(shared) = shared_in_wndproc(hwnd) {
                shared.last_input.set(Some(Instant::now()));
                let vk = VIRTUAL_KEY(wparam.0 as u16);
                let repeat = is_autorepeat(lparam.0);
                if vk == VK_F11 {
                    if !repeat {
                        toggle_fullscreen(hwnd, shared);
                    }
                } else if !repeat {
                    if let Some(k) = vk_to_key(vk) {
                        if input::is_keyup_tracked(k) {
                            shared.keys.borrow_mut().press(k);
                        }
                        shared.send_key(k.to_string());
                    }
                }
            }
            LRESULT(0)
        }
        WM_KEYUP | WM_SYSKEYUP => {
            if let Some(shared) = shared_in_wndproc(hwnd) {
                if let Some(k) = vk_to_key(VIRTUAL_KEY(wparam.0 as u16)) {
                    if input::is_keyup_tracked(k) && shared.keys.borrow_mut().release(k) {
                        shared.send_keyup(k.to_string());
                    }
                }
            }
            if msg == WM_SYSKEYUP {
                default()
            } else {
                LRESULT(0)
            }
        }
        WM_SYSKEYDOWN => {
            // Alt+Enter でフルスクリーン切替。Alt+F4 等その他は既定処理へ。
            if wparam.0 as u16 == VK_RETURN.0 {
                if let Some(shared) = shared_in_wndproc(hwnd) {
                    if !is_autorepeat(lparam.0) {
                        toggle_fullscreen(hwnd, shared);
                    }
                }
                return LRESULT(0);
            }
            default()
        }
        WM_SYSCHAR => {
            // Alt+Enter の文字メッセージを既定処理へ渡すと警告音が鳴る。
            if wparam.0 == 0x0D {
                return LRESULT(0);
            }
            default()
        }
        WM_KILLFOCUS => {
            // フォーカスを失うとキーアップが届かない。押しっぱなし扱いを残さない。
            if let Some(shared) = shared_in_wndproc(hwnd) {
                let held = shared.keys.borrow_mut().release_all();
                for k in held {
                    shared.send_keyup(k);
                }
            }
            default()
        }
        WM_CHAR => {
            if let Some(shared) = shared_in_wndproc(hwnd) {
                if let Some(c) = char::from_u32(wparam.0 as u32) {
                    // 制御文字とスペースは WM_KEYDOWN 側 (Enter/Escape/Control/" ") と
                    // 重複するのでここでは送らない。オートリピートも送らない。
                    if !c.is_control() && c != ' ' && !is_autorepeat(lparam.0) {
                        shared.send_key(c.to_string());
                    }
                }
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            // SAFETY: `BeginPaint`/`EndPaint` は WM_PAINT の中で対にして呼ぶ。`ps` はスタック上で両方の呼び出しの間生きている。
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            if let Some(shared) = shared_in_wndproc(hwnd) {
                let (cw, ch) = shared.client_size.get();
                let vp = shared.viewport();
                shared.with_display(|buf, w, h| blit_dc(hdc, buf, w, h, cw, ch, vp));
            }
            // SAFETY: 上の `BeginPaint` と対。
            let _ = unsafe { EndPaint(hwnd, &ps) };
            LRESULT(0)
        }
        WM_CLOSE => {
            if let Some(shared) = shared_in_wndproc(hwnd) {
                request_close(hwnd, shared);
            } else {
                // SAFETY: 窓スレッドから自分の窓を破棄する。
                let _ = unsafe { DestroyWindow(hwnd) };
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // 破棄された窓へ、遅れたスクリプトスレッドの起こしが飛ばないよう先にスロットを戻す。
            if let Some(shared) = shared_in_wndproc(hwnd) {
                shared.hwnd_slot.store(0, Ordering::SeqCst);
            }
            // SAFETY: メッセージループへ WM_QUIT を投函するだけ。
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => default(),
    }
}

fn register_class(hinstance: HINSTANCE, class_name: &[u16]) -> Result<()> {
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: hinstance,
        // SAFETY: システム定義のカーソル (`IDC_ARROW`) を読み込むだけ（`hInstance` は NULL）。
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        lpszClassName: windows::core::PCWSTR::from_raw(class_name.as_ptr()),
        ..Default::default()
    };
    // SAFETY: `wc` はスタック上で `RegisterClassW` の間生きており、`class_name`（NUL 終端の UTF-16）は呼び出し元が保持する。
    let atom = unsafe { RegisterClassW(&wc) };
    if atom == 0 {
        // 既に登録済み (ERROR_CLASS_ALREADY_EXISTS = 1410) は無視してよい。
        // SAFETY: スレッドのエラー値を読むだけ。
        let err = unsafe { windows::Win32::Foundation::GetLastError() };
        if err.0 != 1410 {
            anyhow::bail!("RegisterClassW に失敗しました: {err:?}");
        }
    }
    Ok(())
}

/// カーソルのあるモニタの (DPI, 作業領域)。取得に失敗したら 96dpi と 1920x1080 相当。
fn monitor_dpi_and_work_area() -> (u32, RECT) {
    // SAFETY: 渡すポインタはいずれもスタック上のローカル（`pt`・`dpi`・`mi`。`mi` は `cbSize` 設定済み）で、各呼び出しの間生きている。
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon: HMONITOR = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut dpi = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi.0, &mut dpi.1);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let work = if GetMonitorInfoW(mon, &mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            }
        };
        (if dpi.0 == 0 { 96 } else { dpi.0 }, work)
    }
}

/// `WS_OVERLAPPEDWINDOW` のリサイズ可能ウィンドウを作成する。クライアントサイズはモニタ DPI
/// 倍（作業領域に収まる最大まで）で作業領域の中央に置く。`visible=false` なら `WS_VISIBLE` を
/// 付けず `ShowWindow` も呼ばない。
fn create_window(
    title: &str,
    game_w: u32,
    game_h: u32,
    visible: bool,
    userdata: *mut c_void,
) -> Result<HWND> {
    // 高 DPI モニタでもピクセル単位でくっきり描画されるようにする。失敗は無視してよい
    // （古い Windows や既に別コンテキストが設定済みの場合など）。
    // SAFETY: プロセス全体の DPI 設定を 1 回変えるだけ（ポインタ引数なし）。
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let hinstance: HINSTANCE = {
        // SAFETY: 自プロセスのモジュールハンドルを取得するだけ（NULL = 呼び出し元の実行ファイル）。
        let hmodule: HMODULE =
            unsafe { GetModuleHandleW(None) }.context("GetModuleHandleW に失敗")?;
        HINSTANCE(hmodule.0)
    };

    let class_name: Vec<u16> = CLASS_NAME.encode_utf16().collect();
    register_class(hinstance, &class_name)?;

    let mut style = WS_OVERLAPPEDWINDOW;
    if visible {
        style |= WS_VISIBLE;
    }

    let (dpi, work) = monitor_dpi_and_work_area();
    // 枠（タイトルバー・境界線）の寸法はクライアントサイズに依らないので、空矩形で測る。
    let mut frame = RECT::default();
    // SAFETY: `frame` はスタック上の RECT。
    unsafe {
        let _ = AdjustWindowRectExForDpi(&mut frame, style, false, WINDOW_EX_STYLE(0), dpi);
    }
    let frame_w = (frame.right - frame.left) as u32;
    let frame_h = (frame.bottom - frame.top) as u32;
    let work_w = (work.right - work.left).max(1) as u32;
    let work_h = (work.bottom - work.top).max(1) as u32;
    let (client_w, client_h) = viewport::initial_client_size(
        game_w,
        game_h,
        dpi,
        work_w.saturating_sub(frame_w),
        work_h.saturating_sub(frame_h),
    );
    let width = (client_w + frame_w) as i32;
    let height = (client_h + frame_h) as i32;
    let x = work.left + (work_w as i32 - width) / 2;
    let y = work.top + (work_h as i32 - height) / 2;

    // SAFETY: クラス名・タイトルは呼び出しの間生きている NUL 終端 UTF-16。`userdata` は呼び出し元（`run_windowed`）の
    // `Box<WindowShared>` のポインタで、窓の `WM_NCCREATE` が `GWLP_USERDATA` へ保存する（回収は `WM_QUIT` 後）。
    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            windows::core::PCWSTR::from_raw(class_name.as_ptr()),
            &HSTRING::from(title),
            style,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(hinstance),
            Some(userdata as *const c_void),
        )
    }
    .context("CreateWindowExW に失敗")?;

    if visible {
        // SAFETY: 直前に生成した窓を表示するだけ。
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
    }

    Ok(hwnd)
}

/// `AppIcon` の指定サイズを Win32 の ICONIMAGE 形式へ変換し `CreateIconFromResourceEx`
/// で `HICON` にする。`.ico`/`.res` の中身と全く同じバイト列を渡せる API のため、
/// 32bpp のアルファチャンネルもタスクバー含め正しく合成される。失敗時は `None`
/// を返し、呼び出し側は `WM_SETICON` を送らない（アイコン設定の失敗で起動自体は
/// 止めない）。
fn create_hicon(icon: &AppIcon, size: u32) -> Option<HICON> {
    let img = icon.image(size);
    let dib = crate::icon::build_dib_icon_image(img.w, img.h, &img.rgba);
    // SAFETY: `dib` は ICONIMAGE 形式のバイト列で、呼び出しの間生きている。返る `HICON` は呼び出し元が `DestroyIcon` で破棄する。
    unsafe {
        CreateIconFromResourceEx(
            &dib,
            true, // fIcon = TRUE（カーソルではなくアイコン）
            0x00030000,
            img.w as i32,
            img.h as i32,
            IMAGE_FLAGS(0),
        )
        .ok()
    }
}

/// ウィンドウ生成直後に `WM_SETICON` でタスクバー/タイトルバーアイコンを設定する。
/// 16x16 (ICON_SMALL) / 32x32 (ICON_BIG) の 2 サイズ。生成した `HICON` は
/// `run_windowed` の終了時に破棄する（返り値をそちらへ持ち回す）。
fn set_window_icon(hwnd: HWND, icon: &AppIcon) -> Vec<HICON> {
    let mut handles = Vec::new();
    if let Some(h) = create_hicon(icon, 16) {
        // SAFETY: `hwnd` は生きている自分の窓、`h` は有効な `HICON`（窓は複製せず参照するので、破棄は窓の終了後に呼び出し元が行う）。
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(0 /* ICON_SMALL */)),
                Some(LPARAM(h.0 as isize)),
            );
        }
        handles.push(h);
    }
    if let Some(h) = create_hicon(icon, 32) {
        // SAFETY: 上と同じ。
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(1 /* ICON_BIG */)),
                Some(LPARAM(h.0 as isize)),
            );
        }
        handles.push(h);
    }
    handles
}

// SAFETY: winmm の `timeBeginPeriod`/`timeEndPeriod`（`UINT` を 1 つ取り `MMRESULT` を返す C 関数、ポインタ引数なし）の
// 宣言。`windows` クレートに無いので OS 境界として直に宣言する。
#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
}

/// フレーム処理（`tick`）が周期をまたいで持つ状態。メッセージループの外側（`run_windowed`）と、OS のモーダルループ
/// 中の `WM_TIMER`（`wndproc`）の両方から回すので、`WindowShared` に置く。
struct FrameLoop {
    scene: SharedScene,
    trans_notify: Arc<Notify>,
    user_rx: crossbeam_channel::Receiver<UserEvent>,
    start: Instant,
    bufs: ComposeBufs,
    frame: Pixmap,
    last_drawn_revision: u64,
    /// 動き（文字送り・tween 等）が終わった直後の周期でも 1 回描く。終わった周期では animating が false で
    /// revision も変わらないので、描かないと最後のグリフ（文字送りの末尾の 」 など）が次の更新まで出ない。
    was_animating: bool,
    last_present: Instant,
    /// `NOIRIA_PROF=1`: フレームごとの合成・転送の所要時間などを noiria.log へ出す（実機の体感の遅さの切り分け用）。
    prof: bool,
    wakes: u32,
}

/// 待ち時間をミリ秒（切り上げ）へ。切り捨てると期限の直前に 0ms 待ちで空回りする。`Duration::MAX` は無期限。
fn wait_ms(wait: Duration) -> u32 {
    if wait == Duration::MAX {
        return u32::MAX;
    }
    wait.as_micros().div_ceil(1000).min(u32::MAX as u128 - 1) as u32
}

/// 1 周期ぶんのフレーム処理: UserEvent のドレイン、閉じる猶予の判定、トランジション完了、描き直し。
/// 戻り値は次に起きるまでの待ち時間。ウィンドウを破棄したときは 0。
///
/// 外側のループが止まる OS のモーダルループ中は `WM_TIMER` から呼ばれる。`SetWindowTextW`/`DestroyWindow` は
/// `wndproc` を同期的に呼び返すので、`frame_loop` の借用はそれらが `tick` に再入しない前提で `tick` の間だけ握る
/// （再入したら何もせず戻る）。
fn tick(hwnd: HWND, shared: &WindowShared) -> Duration {
    let Ok(mut fl) = shared.frame_loop.try_borrow_mut() else {
        return FRAME_INTERVAL;
    };
    let fl = &mut *fl;
    // 投函済みフラグは UserEvent をドレインする前に下ろす（これ以降に送られたイベントは再投函される）。
    shared.wake_posted.store(false, Ordering::SeqCst);
    let mut quit = false;
    while let Ok(ev) = fl.user_rx.try_recv() {
        match ev {
            UserEvent::Redraw => fl.wakes += 1, // 毎周期リビジョンを見て再合成するので明示処理は不要
            UserEvent::SetTitle(title) => {
                // SAFETY: 自分の窓へ、一時 HSTRING（NUL 終端）を渡すだけ。
                let _ = unsafe { SetWindowTextW(hwnd, &HSTRING::from(title)) };
            }
            UserEvent::Quit => quit = true,
        }
    }
    // 閉じる要求にスクリプトが猶予内に応えなかった（無視・ハング）場合は強制終了する。
    let now = Instant::now();
    let (close_expired, close_remaining) = {
        let gate = shared.close_gate.borrow();
        (gate.expired(now), gate.remaining(now))
    };
    if close_expired {
        log::warn!("閉じる要求にスクリプトが応答しなかったため強制終了します");
    }
    if quit || close_expired {
        // SAFETY: 窓スレッドから自分の窓を破棄する。
        let _ = unsafe { DestroyWindow(hwnd) };
        // DestroyWindow は WM_DESTROY を同期的に送出し、その中で PostQuitMessage が呼ばれる。
        // 外側のループが次周期の先頭でそれを回収して抜ける。
        return Duration::ZERO;
    }

    // トランジション完了のステート遷移 + 合成。合成は最短でも FRAME_INTERVAL 間隔（ops のたびに起こされても、
    // 描き直しは 1 周期に 1 回）。シーンのロックは状態の更新とスナップショットの複製の間だけ握り、合成と転送は
    // ロックの外で行う（握ったままだと、メニュー構築のような数百 op のスクリプトが合成のたびに待たされる）。
    let animating;
    let mut frame_due_in = None;
    let lock_wait;
    let snapshot = {
        let t_lock = Instant::now();
        let mut scene = lock_scene(&fl.scene);
        lock_wait = t_lock.elapsed();
        if scene
            .transition
            .as_ref()
            .is_some_and(|t| t.is_finished(now))
        {
            scene.swap_after_transition();
            fl.trans_notify.notify_waiters();
        }
        scene.settle_finished_tweens(now);
        animating = scene.is_animating(now);
        let revision = scene.revision;
        if needs_redraw(revision, fl.last_drawn_revision, animating, fl.was_animating) {
            let due = fl.last_present + FRAME_INTERVAL;
            if now >= due {
                fl.last_drawn_revision = revision;
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
        fl.last_present = now;
        let t_c = Instant::now();
        compose_frame(&scene, now, &mut fl.bufs, &mut fl.frame);
        let compose = t_c.elapsed();
        if fl.prof {
            let (convert, scale, blit, memblit) = present_timed(hwnd, shared, &fl.frame);
            let layers = scene.fore.values().filter(|l| l.visible).count();
            let (cw, ch) = shared.client_size.get();
            crate::prof::frame(
                fl.start,
                compose,
                convert,
                scale,
                blit,
                memblit,
                lock_wait,
                now.elapsed(),
                layers,
                animating,
                fl.wakes,
                (cw, ch),
                shared.last_input.get().map(|t| now.saturating_duration_since(t)),
            );
            fl.wakes = 0;
        } else {
            present(hwnd, shared, &fl.frame);
        }
        fl.was_animating = animating;
    } else if frame_due_in.is_none() {
        fl.was_animating = animating;
    }

    frame_wait(
        animating,
        FRAME_INTERVAL,
        fl.last_present.elapsed(),
        frame_due_in,
        close_remaining,
        Duration::MAX,
    )
}

/// 通常起動のメッセージループ。ウィンドウ生成 → 入力/UserEvent ドレイン →
/// シーン合成/present → `MsgWaitForMultipleObjects` で次のイベントまで待機、
/// を繰り返す。WM_QUIT (WM_DESTROY 経由、または `UserEvent::Quit` / 閉じる猶予切れからの
/// `DestroyWindow`) で終了する。
pub fn run_windowed(p: WindowedParams) -> Result<()> {
    // 既定のタイマー分解能は約 15.6ms で、16ms の待ちが 2 周期（約 31ms）に伸びる。スクリプトスレッドの sleep も同様。
    // SAFETY: プロセス全体のタイマー分解能の要求（`timeEndPeriod(1)` と対。ポインタ引数なし）。
    unsafe {
        timeBeginPeriod(1);
    }
    let shared = Box::new(WindowShared {
        input_tx: p.input_tx,
        game_w: p.game_w,
        game_h: p.game_h,
        client_size: Cell::new((p.game_w, p.game_h)),
        cursor: Cell::new((0.0, 0.0)),
        start: p.start,
        last_frame: RefCell::new(Vec::new()),
        last_frame_size: Cell::new((0, 0)),
        frame_gen: Cell::new(0),
        last_input: Cell::new(None),
        scaled: RefCell::new(ScaleCache::default()),
        frame_loop: RefCell::new(FrameLoop {
            scene: p.scene,
            trans_notify: p.trans_notify,
            user_rx: p.user_rx,
            start: p.start,
            bufs: ComposeBufs::new(p.game_w, p.game_h),
            frame: Pixmap::new(p.game_w, p.game_h),
            last_drawn_revision: u64::MAX,
            was_animating: false,
            last_present: Instant::now() - FRAME_INTERVAL,
            prof: crate::prof::enabled(),
            wakes: 0,
        }),
        wake_posted: p.wake_posted,
        hwnd_slot: p.hwnd_slot.clone(),
        close_gate: RefCell::new(CloseGate::default()),
        keys: RefCell::new(KeyTracker::default()),
        fullscreen: Cell::new(false),
        saved: RefCell::new(None),
    });
    let shared_ptr = Box::into_raw(shared);

    let hwnd = match create_window(
        &p.title,
        p.game_w,
        p.game_h,
        p.visible,
        shared_ptr as *mut c_void,
    ) {
        Ok(h) => h,
        Err(e) => {
            // Box を回収してからエラーを返す（リーク防止）。
            // SAFETY: `shared_ptr` は上の `Box::into_raw` の戻りで、窓の生成に失敗したので他に参照する者はいない（回収は 1 回だけ）。
            drop(unsafe { Box::from_raw(shared_ptr) });
            return Err(e);
        }
    };
    p.hwnd_slot.store(hwnd.0 as isize, Ordering::SeqCst);
    let icon_handles = set_window_icon(hwnd, &p.icon);

    // SAFETY: `shared_ptr` は `Box::into_raw` で得た有効なポインタで、ループを抜けた後の `Box::from_raw` まで解放されない。
    // 以後は共有参照としてのみ使い（`wndproc` も同じ）、内部可変性は同一スレッド専用の `Cell`/`RefCell` に限る。
    let shared_ref = unsafe { &*shared_ptr };

    'outer: loop {
        // ウィンドウメッセージをドレイン。
        let mut msg = MSG::default();
        loop {
            // SAFETY: `msg` はスタック上の MSG。窓スレッド（このスレッド）のキューを読む。
            let has_msg = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool();
            if !has_msg {
                break;
            }
            if msg.message == WM_QUIT {
                break 'outer;
            }
            // SAFETY: 直前に `PeekMessageW` が埋めた有効な MSG を渡す。
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        let wait = tick(hwnd, shared_ref);
        // SAFETY: ハンドル配列なし（`None`）で、メッセージ到着かタイムアウトまで待つだけ。
        unsafe {
            MsgWaitForMultipleObjects(None, false, wait_ms(wait), QS_ALLINPUT);
        }
    }

    // SAFETY: `timeBeginPeriod(1)` と対。
    unsafe {
        timeEndPeriod(1);
    }
    for h in icon_handles {
        // SAFETY: `create_hicon` が作った `HICON`。窓は既に破棄済みで、各ハンドルはここで 1 回だけ破棄する。
        unsafe {
            let _ = DestroyIcon(h);
        }
    }
    // SAFETY: `Box::into_raw` で得たポインタを回収する（唯一の回収）。窓は破棄済み（`WM_QUIT`）で `wndproc` はもう呼ばれず、
    // `shared_ref` もこの後は使わない。
    drop(unsafe { Box::from_raw(shared_ptr) });
    Ok(())
}

#[cfg(test)]
mod tests {
    //! 不可視ウィンドウ統合テスト。可視ウィンドウは一切出さない
    //! (`visible: false` → `WS_VISIBLE` を付けず `ShowWindow` も呼ばない。フルスクリーン切替も
    //! `WS_VISIBLE` の有無は保つ)。別スレッドから `PostMessageW` で入力を注入し、
    //! `InputEvent` への変換・座標の逆変換・クリーンな終了をアサートする。
    //!
    //! 初期クライアントサイズはモニタ DPI に依存するため、座標を検証するテストは先に
    //! `set_client_size` で明示的なサイズへ揃える。

    use super::*;
    use std::sync::{Arc, Mutex};
    use std::thread;

    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
    use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, WINDOW_STYLE};

    use crate::bridge::UserEvent;

    /// OS のモーダルループ（タイトルバーのドラッグなど）を模すテスト専用メッセージ。wparam = 継続ミリ秒。
    pub(super) const WM_TEST_MODAL_PUMP: u32 = WM_APP + 1;

    /// `DefWindowProc` のモーダルループと同じ形（`WM_ENTERSIZEMOVE` を送り、`PeekMessage`/`DispatchMessage` だけを回し、
    /// `WM_EXITSIZEMOVE` を送る）を `ms` ミリ秒だけ回す。この間、外側の `run_windowed` のループは止まる。
    pub(super) fn modal_pump(hwnd: HWND, ms: u64) {
        // SAFETY: 窓スレッド上で自分の窓へ同期送信するだけ（`wndproc` を直接呼ぶ）。
        unsafe {
            SendMessageW(hwnd, WM_ENTERSIZEMOVE, Some(WPARAM(0)), Some(LPARAM(0)));
        }
        let deadline = Instant::now() + Duration::from_millis(ms);
        let mut msg = MSG::default();
        while Instant::now() < deadline {
            // SAFETY: `msg` はスタック上の MSG。窓スレッド（このスレッド）のキューを読む。
            if unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                if msg.message == WM_QUIT {
                    // 本物のモーダルループと同じく、WM_QUIT は外側のループへ返して抜ける。
                    // SAFETY: メッセージループへ WM_QUIT を再投函するだけ。
                    unsafe { PostQuitMessage(msg.wParam.0 as i32) };
                    break;
                }
                // SAFETY: 直前に `PeekMessageW` が埋めた有効な MSG。
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        // SAFETY: 上の `WM_ENTERSIZEMOVE` と対（破棄済みの窓なら何も起きない）。
        unsafe {
            SendMessageW(hwnd, WM_EXITSIZEMOVE, Some(WPARAM(0)), Some(LPARAM(0)));
        }
    }

    const GAME_W: u32 = 320;
    const GAME_H: u32 = 240;

    fn make_lparam(x: i16, y: i16) -> LPARAM {
        LPARAM((((y as u16 as u32) << 16) | (x as u16 as u32)) as i32 as isize)
    }

    fn wait_for_hwnd(slot: &AtomicIsize) -> HWND {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let raw = slot.load(Ordering::SeqCst);
            if raw != 0 {
                return HWND(raw as *mut c_void);
            }
            assert!(
                Instant::now() < deadline,
                "ウィンドウ生成がタイムアウトしました"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn recv_within(rx: &mut UnboundedReceiver<InputEvent>, timeout: Duration) -> InputEvent {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(ev) = rx.try_recv() {
                return ev;
            }
            assert!(
                Instant::now() < deadline,
                "InputEvent を受信できませんでした"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    struct Harness {
        handle: thread::JoinHandle<Result<()>>,
        hwnd: HWND,
        input_rx: UnboundedReceiver<InputEvent>,
        user_tx: crossbeam_channel::Sender<UserEvent>,
        scene: SharedScene,
        wake_posted: Arc<AtomicBool>,
        hwnd_slot: Arc<AtomicIsize>,
    }

    fn spawn_window() -> Harness {
        let scene = Arc::new(Mutex::new(noiria_core::scene::Scene::new(GAME_W, GAME_H)));
        let scene_for_test = scene.clone();
        let wake_posted = Arc::new(AtomicBool::new(false));
        let (input_tx, input_rx) = unbounded_channel();
        let trans_notify = Arc::new(Notify::new());
        let (user_tx, user_rx) = crossbeam_channel::unbounded();
        let hwnd_slot = Arc::new(AtomicIsize::new(0));
        let params = WindowedParams {
            title: "noiria invisible test".to_string(),
            game_w: GAME_W,
            game_h: GAME_H,
            scene,
            input_tx,
            trans_notify,
            start: Instant::now(),
            user_rx,
            hwnd_slot: hwnd_slot.clone(),
            wake_posted: wake_posted.clone(),
            icon: crate::icon::AppIcon::from_optional_png(None),
            // 画面には何も表示しない。
            visible: false,
        };
        let handle = thread::spawn(move || run_windowed(params));
        let hwnd = wait_for_hwnd(&hwnd_slot);
        Harness {
            handle,
            hwnd,
            input_rx,
            user_tx,
            scene: scene_for_test,
            wake_posted,
            hwnd_slot,
        }
    }

    impl Harness {
        fn post(&self, msg: u32, wparam: usize, lparam: LPARAM) {
            // SAFETY: 窓へメッセージを投函するだけ（破棄済みでも Err を返すだけ）。
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), msg, WPARAM(wparam), lparam);
            }
        }

        fn recv(&mut self) -> InputEvent {
            recv_within(&mut self.input_rx, Duration::from_secs(3))
        }

        /// click だけを待つ。ジオメトリ変更でカーソル下の窓が動くと OS が WM_MOUSEMOVE を合成して
        /// 送ってくることがあり（実機で確認）、その move は読み飛ばす。
        fn recv_click(&mut self) -> InputEvent {
            loop {
                let ev = self.recv();
                if ev.kind == "click" {
                    return ev;
                }
            }
        }

        /// 次のイベントが来ない（キューが空のまま）ことを短時間で確認する。OS が合成する move は無視する。
        fn assert_no_event_pending(&mut self) {
            thread::sleep(Duration::from_millis(150));
            while let Ok(ev) = self.input_rx.try_recv() {
                assert_eq!(ev.kind, "move", "想定外の InputEvent が届いています: {ev:?}");
            }
        }

        fn quit(self) {
            let _ = self.user_tx.send(UserEvent::Quit);
            post_wake(self.hwnd.0 as isize, &self.wake_posted);
            self.handle
                .join()
                .expect("メッセージループスレッドが panic しました")
                .expect("run_windowed がエラーを返しました");
        }
    }

    fn client_rect(hwnd: HWND) -> (i32, i32) {
        let mut rc = RECT::default();
        // SAFETY: `rc` はスタック上の RECT（無効な窓なら Err を返すだけ）。
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        (rc.right - rc.left, rc.bottom - rc.top)
    }

    /// クライアントサイズを `w`×`h` にして、実際にそうなるまで待つ。
    fn set_client_size(hwnd: HWND, w: i32, h: i32) {
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        // SAFETY: `rc` はスタック上の RECT。テスト用の自分の窓へのサイズ変更要求。
        unsafe {
            let style = WINDOW_STYLE(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32);
            let _ = AdjustWindowRectExForDpi(
                &mut rc,
                style,
                false,
                WINDOW_EX_STYLE(0),
                GetDpiForWindow(hwnd),
            );
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                rc.right - rc.left,
                rc.bottom - rc.top,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while client_rect(hwnd) != (w, h) {
            assert!(
                Instant::now() < deadline,
                "クライアントサイズを {w}x{h} にできません (現在 {:?})",
                client_rect(hwnd)
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn style_of(hwnd: HWND) -> u32 {
        // SAFETY: スタイルの読み出しだけ。
        unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 }
    }

    fn join_within(handle: thread::JoinHandle<Result<()>>, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while !handle.is_finished() {
            assert!(Instant::now() < deadline, "ウィンドウスレッドが終了しません");
            thread::sleep(Duration::from_millis(10));
        }
        handle.join().unwrap().unwrap();
    }

    #[test]
    fn vk_mapping_and_autorepeat_flag() {
        assert_eq!(vk_to_key(VK_SHIFT), Some("Shift"));
        assert_eq!(vk_to_key(VK_CONTROL), Some("Control"));
        assert_eq!(vk_to_key(VK_RETURN), Some("Enter"));
        assert_eq!(vk_to_key(VK_SPACE), Some(" "));
        assert_eq!(vk_to_key(VK_F11), None, "F11 はホストが消費しスクリプトへ渡さない");
        assert!(!is_autorepeat(0x0001_0001));
        assert!(is_autorepeat(0x4001_0001));
    }

    #[test]
    fn invisible_window_input_roundtrip_and_clean_quit() {
        let mut h = spawn_window();
        // DPI 依存の初期サイズに左右されないよう、論理解像度と同じクライアントサイズへ揃える。
        set_client_size(h.hwnd, GAME_W as i32, GAME_H as i32);

        // WM_LBUTTONDOWN をクライアント座標 (100, 50) へ注入する。present() は
        // ウィンドウ生成直後の最初のループ周期で既に最低 1 回実行されている
        // （revision != last_drawn_revision の初期条件により、隠れていても
        // GetDC/StretchDIBits が成功することを確認済み）。
        h.post(WM_LBUTTONDOWN, 0, make_lparam(100, 50));
        let click = h.recv();
        assert_eq!(click.kind, "click");
        assert_eq!(click.button, Some("left"));
        assert!((click.x - 100.0).abs() < 5.0, "x={}", click.x);
        assert!((click.y - 50.0).abs() < 5.0, "y={}", click.y);

        // WM_KEYDOWN(VK_F5) → "F5"。
        h.post(WM_KEYDOWN, VK_F5.0 as usize, LPARAM(0));
        let key = h.recv();
        assert_eq!(key.kind, "key");
        assert_eq!(key.key.as_deref(), Some("F5"));

        // WM_MOUSEWHEEL。lParam はスクリーン座標が要求されるので
        // ClientToScreen で変換した点を使う（上方向 = 正、120/120.0 = 1.0）。
        let mut origin = POINT { x: 0, y: 0 };
        // SAFETY: `origin` はスタック上の POINT。
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(h.hwnd, &mut origin);
        }
        let wheel_screen_x = (origin.x + 20) as i16;
        let wheel_screen_y = (origin.y + 30) as i16;
        let wparam_wheel = (120u32 << 16) as usize; // GET_WHEEL_DELTA_WPARAM = +120（上方向）
        h.post(
            WM_MOUSEWHEEL,
            wparam_wheel,
            make_lparam(wheel_screen_x, wheel_screen_y),
        );
        let wheel = h.recv();
        assert_eq!(wheel.kind, "wheel");
        assert_eq!(wheel.delta, Some(1.0));
        assert!((wheel.x - 20.0).abs() < 5.0, "x={}", wheel.x);
        assert!((wheel.y - 30.0).abs() < 5.0, "y={}", wheel.y);

        // UserEvent::Quit → メッセージループを起こして (PostMessageW WM_APP)
        // クリーンに終了させ、スレッドを join する。
        h.quit();
    }

    /// 起動直後（リサイズ前）のクライアントサイズは DPI 倍率でも論理解像度のアスペクト比を保ち、
    /// 初期の `client_size`（WM_SIZE 経由）が正しければ中央クリックがゲーム中央に写る。
    #[test]
    fn initial_window_keeps_aspect_and_center_click_maps_to_game_center() {
        let mut h = spawn_window();
        let (cw, ch) = client_rect(h.hwnd);
        assert!(cw >= GAME_W as i32 && ch >= GAME_H as i32, "{cw}x{ch}");
        assert!(
            (cw as f64 / ch as f64 - GAME_W as f64 / GAME_H as f64).abs() < 0.01,
            "アスペクト比が崩れている: {cw}x{ch}"
        );
        h.post(WM_LBUTTONDOWN, 0, make_lparam((cw / 2) as i16, (ch / 2) as i16));
        let c = h.recv_click();
        assert!(
            (c.x - GAME_W as f64 / 2.0).abs() <= 1.5 && (c.y - GAME_H as f64 / 2.0).abs() <= 1.5,
            "({}, {}) / client {cw}x{ch}",
            c.x,
            c.y
        );
        h.quit();
    }

    /// 表示倍率とレターボックスの逆変換が、実ウィンドウのクライアントサイズ変更に追随する。
    #[test]
    fn mouse_coordinates_follow_scaling_and_letterbox() {
        let mut h = spawn_window();

        // 2 倍（640x480）: クライアント中央 = ゲーム中央。
        set_client_size(h.hwnd, 640, 480);
        h.post(WM_LBUTTONDOWN, 0, make_lparam(320, 240));
        let c = h.recv_click();
        assert_eq!((c.x, c.y), (160.0, 120.0));
        h.post(WM_MOUSEMOVE, 0, make_lparam(640 - 1, 0));
        let m = h.recv();
        assert!((m.x - 319.5).abs() < 1e-9 && m.y == 0.0, "{} {}", m.x, m.y);

        // 横長（800x240）: 左右に 240px の黒帯 → ゲームは x=240..560。
        set_client_size(h.hwnd, 800, 240);
        h.post(WM_LBUTTONDOWN, 0, make_lparam(240 + 160, 120));
        let c = h.recv_click();
        assert_eq!((c.x, c.y), (160.0, 120.0));
        h.post(WM_LBUTTONDOWN, 0, make_lparam(240, 0));
        let c = h.recv_click();
        assert_eq!((c.x, c.y), (0.0, 0.0));
        // 黒帯上はゲーム範囲外（クランプしない）。
        h.post(WM_LBUTTONDOWN, 0, make_lparam(10, 120));
        let c = h.recv_click();
        assert!(c.x < 0.0, "x={}", c.x);

        // 縦長（320x480）: 上下 120px の黒帯 → ゲームは y=120..360。
        set_client_size(h.hwnd, 320, 480);
        h.post(WM_LBUTTONDOWN, 0, make_lparam(160, 120 + 120));
        let c = h.recv_click();
        assert_eq!((c.x, c.y), (160.0, 120.0));
        h.quit();
    }

    #[test]
    fn f11_and_alt_enter_toggle_borderless_fullscreen() {
        let mut h = spawn_window();
        set_client_size(h.hwnd, GAME_W as i32, GAME_H as i32);
        let windowed_style = style_of(h.hwnd);
        assert_ne!(windowed_style & WS_OVERLAPPEDWINDOW.0, 0);
        assert_eq!(windowed_style & WS_VISIBLE.0, 0, "不可視テスト");

        // SAFETY: 値を返すだけ。
        let mon = unsafe { MonitorFromWindow(h.hwnd, MONITOR_DEFAULTTONEAREST) };
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `mi` は `cbSize` 設定済みのスタック上の構造体。
        unsafe {
            let _ = GetMonitorInfoW(mon, &mut mi);
        }
        let (mw, mh) = (
            mi.rcMonitor.right - mi.rcMonitor.left,
            mi.rcMonitor.bottom - mi.rcMonitor.top,
        );

        // F11: ボーダーレス化してモニタ全体を覆う。F11 自体はスクリプトへ送らない。
        h.post(WM_KEYDOWN, VK_F11.0 as usize, LPARAM(0));
        let deadline = Instant::now() + Duration::from_secs(3);
        while client_rect(h.hwnd) != (mw, mh) {
            assert!(Instant::now() < deadline, "フルスクリーン化されません");
            thread::sleep(Duration::from_millis(10));
        }
        let fs_style = style_of(h.hwnd);
        assert_eq!(fs_style & WS_OVERLAPPEDWINDOW.0, 0, "枠が残っている");
        assert_ne!(fs_style & WS_POPUP.0, 0);
        assert_eq!(fs_style & WS_VISIBLE.0, 0, "切替で可視化してはならない");

        // オートリピートの F11 は無視（トグルし直さない）。
        h.post(WM_KEYDOWN, VK_F11.0 as usize, LPARAM(KEY_PREV_STATE));
        thread::sleep(Duration::from_millis(150));
        assert_eq!(client_rect(h.hwnd), (mw, mh));

        // フルスクリーン中のマウス座標はレターボックスを考慮する（中央 = ゲーム中央）。
        h.post(WM_LBUTTONDOWN, 0, make_lparam((mw / 2) as i16, (mh / 2) as i16));
        let c = h.recv_click();
        assert!(
            (c.x - GAME_W as f64 / 2.0).abs() <= 2.0 && (c.y - GAME_H as f64 / 2.0).abs() <= 2.0,
            "({}, {})",
            c.x,
            c.y
        );

        // Alt+Enter（WM_SYSKEYDOWN、lParam bit 29 = Alt 押下）で元に戻る。
        h.post(WM_SYSKEYDOWN, VK_RETURN.0 as usize, LPARAM(1 << 29));
        let deadline = Instant::now() + Duration::from_secs(3);
        while client_rect(h.hwnd) != (GAME_W as i32, GAME_H as i32) {
            assert!(Instant::now() < deadline, "元のサイズへ戻りません");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(style_of(h.hwnd), windowed_style);
        h.assert_no_event_pending();
        h.quit();
    }

    #[test]
    fn key_autorepeat_is_suppressed_and_keyup_is_sent() {
        let mut h = spawn_window();

        h.post(WM_KEYDOWN, VK_CONTROL.0 as usize, LPARAM(1));
        let down = h.recv();
        assert_eq!((down.kind, down.key.as_deref()), ("key", Some("Control")));
        // オートリピート（lParam bit 30）は送らない。
        for _ in 0..3 {
            h.post(WM_KEYDOWN, VK_CONTROL.0 as usize, LPARAM(1 | KEY_PREV_STATE));
        }
        // 印字文字（WM_CHAR）のリピートも送らない。初回は送る。
        h.post(WM_CHAR, 'a' as usize, LPARAM(1));
        h.post(WM_CHAR, 'a' as usize, LPARAM(1 | KEY_PREV_STATE));
        // 後続の別キーが来るまでの間に、リピート分が混入していないこと。
        let a = h.recv();
        assert_eq!((a.kind, a.key.as_deref()), ("key", Some("a")));
        h.post(WM_KEYUP, VK_CONTROL.0 as usize, LPARAM(1 | (1 << 30) | (1 << 31)));
        let up = h.recv();
        assert_eq!((up.kind, up.key.as_deref()), ("keyup", Some("Control")));

        for (vk, name) in [(VK_SHIFT, "Shift"), (VK_RETURN, "Enter"), (VK_SPACE, " ")] {
            h.post(WM_KEYDOWN, vk.0 as usize, LPARAM(1));
            h.post(WM_KEYUP, vk.0 as usize, LPARAM(1));
            let d = h.recv();
            let u = h.recv();
            assert_eq!((d.kind, d.key.as_deref()), ("key", Some(name)));
            assert_eq!((u.kind, u.key.as_deref()), ("keyup", Some(name)));
        }
        h.assert_no_event_pending();
        h.quit();
    }

    #[test]
    fn losing_focus_releases_held_keys() {
        let mut h = spawn_window();
        h.post(WM_KEYDOWN, VK_CONTROL.0 as usize, LPARAM(1));
        assert_eq!(h.recv().key.as_deref(), Some("Control"));
        h.post(WM_KILLFOCUS, 0, LPARAM(0));
        let up = h.recv();
        assert_eq!((up.kind, up.key.as_deref()), ("keyup", Some("Control")));
        // 既に離した扱いなので 2 度目の失焦では何も出ない。
        h.post(WM_KILLFOCUS, 0, LPARAM(0));
        h.assert_no_event_pending();
        h.quit();
    }

    /// 閉じる操作は即 Destroy せずスクリプトへ `WindowClose` を送る。スクリプトが猶予内に
    /// 終了要求を返せば（`UserEvent::Quit`）正常終了する。
    #[test]
    fn window_close_is_forwarded_and_script_can_finish_the_exit() {
        let mut h = spawn_window();
        h.post(WM_CLOSE, 0, LPARAM(0));
        let ev = h.recv();
        assert_eq!((ev.kind, ev.key.as_deref()), ("key", Some("WindowClose")));
        thread::sleep(Duration::from_millis(300));
        assert!(!h.handle.is_finished(), "スクリプトの返答前に閉じてはならない");
        h.quit();
    }

    #[test]
    fn window_close_without_script_response_is_forced_after_grace() {
        let mut h = spawn_window();
        let t0 = Instant::now();
        h.post(WM_CLOSE, 0, LPARAM(0));
        assert_eq!(h.recv().key.as_deref(), Some("WindowClose"));
        thread::sleep(Duration::from_millis(700));
        assert!(!h.handle.is_finished(), "猶予内に閉じてはならない");
        join_within(h.handle, Duration::from_secs(4));
        let elapsed = t0.elapsed();
        assert!(
            elapsed >= input::CLOSE_GRACE - Duration::from_millis(100),
            "猶予より早く閉じた: {elapsed:?}"
        );
    }

    #[test]
    fn window_close_with_dead_script_destroys_immediately() {
        let h = spawn_window();
        // スクリプトスレッド死亡の再現: 入力の受信口を捨てる。
        drop(h.input_rx);
        let t0 = Instant::now();
        // SAFETY: 窓へ投函するだけ。
        unsafe {
            let _ = PostMessageW(Some(h.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        join_within(h.handle, Duration::from_millis(1500));
        assert!(t0.elapsed() < input::CLOSE_GRACE);
    }

    #[test]
    fn wait_ms_rounds_up_and_maps_max_to_infinite() {
        assert_eq!(wait_ms(Duration::ZERO), 0);
        assert_eq!(wait_ms(Duration::from_micros(1)), 1, "切り捨てると期限の直前に 0ms 待ちで空回りする");
        assert_eq!(wait_ms(Duration::from_micros(9_300)), 10);
        assert_eq!(wait_ms(Duration::from_millis(16)), 16);
        assert_eq!(wait_ms(Duration::MAX), u32::MAX);
    }

    /// 長さ `ms` のクロスフェードを開始する（`is_finished` で完了判定される）。
    fn start_transition(h: &Harness, ms: u64) {
        use noiria_core::transition::{TransMethod, Transition};
        h.scene.lock().unwrap().transition = Some(Transition {
            method: TransMethod::Crossfade,
            start: Instant::now(),
            duration: Duration::from_millis(ms),
            skip: false,
        });
    }

    /// 指摘 6: タイトルバーのドラッグ等の OS モーダルループ中は外側のループが止まる。`WM_TIMER` でフレーム処理を回さないと
    /// トランジションが完了しない（`swap_after_transition` / `trans_notify` が止まり、スクリプトの `trans.wait` も止まる）。
    #[test]
    fn modal_loop_keeps_transitions_completing() {
        let h = spawn_window();
        start_transition(&h, 150);
        let t0 = Instant::now();
        h.post(WM_TEST_MODAL_PUMP, 1500, LPARAM(0));
        let done_at = loop {
            if h.scene.lock().unwrap().transition.is_none() {
                break t0.elapsed();
            }
            assert!(t0.elapsed() < Duration::from_secs(4), "トランジションが完了しません");
            thread::sleep(Duration::from_millis(5));
        };
        assert!(
            done_at < Duration::from_millis(1000),
            "モーダルループ（1500ms）が終わるまで完了しなかった: {done_at:?}"
        );
        h.quit();
    }

    /// 指摘 6: モーダルループ中でも `UserEvent::Quit`（と閉じる猶予切れ）は処理される。
    #[test]
    fn modal_loop_still_processes_quit() {
        let h = spawn_window();
        h.post(WM_TEST_MODAL_PUMP, 3000, LPARAM(0));
        thread::sleep(Duration::from_millis(100));
        let t0 = Instant::now();
        let _ = h.user_tx.send(UserEvent::Quit);
        join_within(h.handle, Duration::from_millis(1500));
        assert!(t0.elapsed() < Duration::from_millis(1500));
    }

    /// 指摘 10: 起こしの投函済みフラグは窓ごと。別窓のフラグが立ったままでも、この窓の起こしは届く。
    #[test]
    fn wake_flags_are_per_window() {
        let a = spawn_window();
        let b = spawn_window();
        // A の起こしが「投函済みで未取り込み」のまま。プロセス全体で 1 つのフラグだと B の起こしがこれに抑止される。
        a.wake_posted.store(true, Ordering::SeqCst);
        let _ = b.user_tx.send(UserEvent::Quit);
        post_wake(b.hwnd.0 as isize, &b.wake_posted);
        join_within(b.handle, Duration::from_secs(3));
        a.wake_posted.store(false, Ordering::SeqCst);
        a.quit();
    }

    /// 指摘 10: 窓の破棄後は hwnd スロットが 0 へ戻り、遅れた起こしは死んだ HWND へ飛ばない。
    #[test]
    fn hwnd_slot_is_cleared_after_destroy_and_late_wakes_are_noops() {
        let h = spawn_window();
        let slot = h.hwnd_slot.clone();
        let posted = h.wake_posted.clone();
        assert_ne!(slot.load(Ordering::SeqCst), 0);
        h.quit();
        assert_eq!(slot.load(Ordering::SeqCst), 0);
        post_wake(slot.load(Ordering::SeqCst), &posted);
        assert!(!posted.load(Ordering::SeqCst), "hwnd 0 では投函も旗の更新もしない");
    }

    /// 指摘 12: `blit_dc` は `buf` が `fw`×`fh` に足りなければ描かない（範囲外を読まない）。
    #[test]
    fn blit_dc_skips_a_short_buffer() {
        let h = spawn_window();
        // SAFETY: テスト用の自分の窓の DC を取得して必ず解放する。`blit_dc` は足りないバッファでは何も読まずに戻る。
        unsafe {
            let hdc = GetDC(Some(h.hwnd));
            let vp = Viewport { x: 0, y: 0, w: 320, h: 240 };
            blit_dc(hdc, &[0u32; 10], 320, 240, 320, 240, vp);
            ReleaseDC(Some(h.hwnd), hdc);
        }
        h.quit();
    }
}
