//! 実ウィンドウ経路の計測用ドライバ（開発用。`--bench-window WxH --bench-script <file>`）。不可視ウィンドウ（画面には何も出さず、
//! 音も鳴らさない）で本物の描画ループ・スクリプトスレッドを動かし、`--autotest-script` と同じ DSL を実時間で
//! `PostMessageW` として注入する。`shot <名前>` は何も撮らず、`NOIRIA_PROF` のログに目印（`mark=<名前>`）を出す。
//! ログの読み方は `docs/distribution.md` の「実機の遅さを切り分ける」を参照。

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowLongPtrW, PostMessageW, SetWindowPos, GWL_STYLE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOZORDER, WINDOW_EX_STYLE, WINDOW_STYLE, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_MOUSEMOVE,
    WM_RBUTTONDOWN,
};

use crate::autotest_script::{Button, Step};
use crate::bridge::UserEvent;

const WINDOW_WAKE: u32 = 0x8000; // WM_APP

fn lparam(x: f64, y: f64) -> LPARAM {
    let (x, y) = (x.round() as i32 as i16, y.round() as i32 as i16);
    LPARAM((((y as u16 as u32) << 16) | (x as u16 as u32)) as i32 as isize)
}

fn vk(name: &str) -> Option<usize> {
    Some(match name {
        "Backspace" => 0x08,
        "Tab" => 0x09,
        "Escape" => 0x1B,
        "Enter" => 0x0D,
        "Control" => 0x11,
        "Shift" => 0x10,
        " " => 0x20,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "End" => 0x23,
        "Home" => 0x24,
        "ArrowLeft" => 0x25,
        "ArrowUp" => 0x26,
        "ArrowRight" => 0x27,
        "ArrowDown" => 0x28,
        "Delete" => 0x2E,
        "F1" => 0x70,
        "F2" => 0x71,
        "F3" => 0x72,
        "F4" => 0x73,
        "F5" => 0x74,
        "F6" => 0x75,
        "F7" => 0x76,
        "F8" => 0x77,
        "F9" => 0x78,
        "F10" => 0x79,
        "F12" => 0x7B,
        _ => return None,
    })
}

fn set_client_size(hwnd: HWND, w: i32, h: i32) {
    let mut rc = RECT { left: 0, top: 0, right: w, bottom: h };
    // SAFETY: `rc` はスタック上の RECT。`hwnd` は描画ループが公開した窓で、破棄済みなら各呼び出しは Err/無害な値を返すだけ。
    unsafe {
        let style = WINDOW_STYLE(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32);
        let _ = AdjustWindowRectExForDpi(&mut rc, style, false, WINDOW_EX_STYLE(0), GetDpiForWindow(hwnd));
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
    loop {
        let mut cr = RECT::default();
        // SAFETY: `cr` はスタック上の RECT。
        unsafe {
            let _ = GetClientRect(hwnd, &mut cr);
        }
        if (cr.right - cr.left, cr.bottom - cr.top) == (w, h) || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// ドライバスレッドを起動する。スクリプトが終わったら `UserEvent::Quit` で描画ループを終わらせる。
pub(crate) fn spawn(
    hwnd_slot: Arc<AtomicIsize>,
    user_tx: crossbeam_channel::Sender<UserEvent>,
    start: Instant,
    size: (u32, u32),
    game_w: u32,
    steps: Vec<Step>,
) {
    std::thread::spawn(move || {
        let hwnd = loop {
            let raw = hwnd_slot.load(Ordering::SeqCst);
            if raw != 0 {
                break HWND(raw as *mut std::ffi::c_void);
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        set_client_size(hwnd, size.0 as i32, size.1 as i32);
        let scale = size.0 as f64 / game_w as f64;
        // SAFETY: 窓へメッセージを投函するだけ（破棄済みの窓なら Err を返すだけ）。
        let post = |msg: u32, wp: usize, lp: LPARAM| unsafe {
            let _ = PostMessageW(Some(hwnd), msg, WPARAM(wp), lp);
        };
        for step in steps {
            match step {
                Step::Wait(ms) => std::thread::sleep(Duration::from_millis(ms)),
                Step::Move(x, y) => post(WM_MOUSEMOVE, 0, lparam(x * scale, y * scale)),
                Step::Click(x, y, b) => {
                    post(WM_MOUSEMOVE, 0, lparam(x * scale, y * scale));
                    crate::prof::mark(start, &format!("click {x:.0},{y:.0}"));
                    let msg = if b == Button::Left { WM_LBUTTONDOWN } else { WM_RBUTTONDOWN };
                    post(msg, 0, lparam(x * scale, y * scale));
                }
                Step::Key(k) => {
                    if let Some(code) = vk(&k) {
                        crate::prof::mark(start, &format!("key {k}"));
                        post(WM_KEYDOWN, code, LPARAM(1));
                    }
                }
                Step::KeyUp(k) => {
                    if let Some(code) = vk(&k) {
                        post(WM_KEYUP, code, LPARAM(1 | (1 << 30) | (1 << 31)));
                    }
                }
                Step::Shot(name) => crate::prof::mark(start, &name),
            }
        }
        std::thread::sleep(Duration::from_millis(500));
        let _ = user_tx.send(UserEvent::Quit);
        post(WINDOW_WAKE, 0, LPARAM(0));
    });
}
