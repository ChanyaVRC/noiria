//! 致命エラーの可視化。
//!
//! 配布ビルド（Windows、`windows_subsystem="windows"`）は stderr もコンソールも無いため、
//! game.pak 不在・起動失敗・スクリプトスレッドの致命終了・panic を黙って落とすとユーザーには
//! 「起動しない」としか見えない。そこで `MessageBoxW` で内容を出し、ログにも残す。
//!
//! ダイアログは **ヘッドレス（`--autotest*`）・`cargo test`・`NOIRIA_NO_DIALOG` 設定時には
//! 絶対に出さない**（CI/自動検証をブロックしないため）。その場合は stderr とログのみ。
//! Linux は常に stderr + ログ。

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static HEADLESS: AtomicBool = AtomicBool::new(false);
static SCRIPT_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// ダイアログを出してよいか。配布ビルド（`dist`）の Windows だけが対象で、それ以外では
/// stderr + ログのみ（コンソールのある開発ビルドでは端末に出れば足りる）。
pub fn should_show_dialog(headless: bool, dist_windows: bool, no_dialog_env: bool) -> bool {
    dist_windows && !headless && !no_dialog_env
}

fn dialog_allowed() -> bool {
    should_show_dialog(
        HEADLESS.load(Ordering::SeqCst) || cfg!(test),
        cfg!(all(windows, feature = "dist")),
        std::env::var_os("NOIRIA_NO_DIALOG").is_some_and(|v| !v.is_empty()),
    )
}

pub fn set_headless(headless: bool) {
    HEADLESS.store(headless, Ordering::SeqCst);
}

/// スクリプトスレッドの致命終了を記録する（スクリプトスレッド側から呼ぶ）。ウィンドウは
/// 続く `UserEvent::Quit` で閉じ、メインスレッドが `take_script_error` で拾って報告する
/// （ダイアログをスクリプトスレッドから出すと、閉じかけのウィンドウと競合するため）。
pub fn record_script_error(msg: String) {
    log::error!("スクリプト実行エラー: {msg}");
    *SCRIPT_ERROR.lock().unwrap() = Some(msg);
}

pub fn take_script_error() -> Option<String> {
    SCRIPT_ERROR.lock().unwrap().take()
}

/// `catch_unwind` で捕まえた panic ペイロードの文字列化。
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    let msg = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(不明な panic)".to_string());
    format!("スクリプトスレッドが panic しました: {msg}")
}

/// 致命エラーを報告する: ログ + stderr、許可されていれば `MessageBoxW`。
pub fn report(title: &str, message: &str) {
    log::error!("{title}: {message}");
    let _ = writeln!(std::io::stderr(), "Error: {title}: {message}");
    if dialog_allowed() {
        #[allow(unused_mut)]
        let mut text = message.to_string();
        #[cfg(feature = "dist")]
        if let Some(path) = crate::dist_logger::log_path() {
            text.push_str(&format!("\n\n詳細はログを参照してください:\n{}", path.display()));
        }
        message_box(title, &text);
    }
}

#[cfg(windows)]
fn message_box(title: &str, text: &str) {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND,
    };
    // SAFETY: 所有者窓なし（`None`）のモーダルダイアログ。`HSTRING` は NUL 終端で、呼び出しの間生きている。
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            &HSTRING::from(title),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

#[cfg(not(windows))]
fn message_box(_title: &str, _text: &str) {}

/// panic をログ・stderr・ダイアログへ出すフックを入れる。既定フック（stderr への
/// バックトレース）も維持する。
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("?");
        report(
            "Noiria - 予期しないエラー",
            &format!("スレッド '{name}' で panic しました:\n{info}"),
        );
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_only_for_dist_windows_when_interactive() {
        assert!(should_show_dialog(false, true, false));
        assert!(!should_show_dialog(true, true, false), "ヘッドレスでは出さない");
        assert!(!should_show_dialog(false, true, true), "NOIRIA_NO_DIALOG で抑止");
        assert!(!should_show_dialog(false, false, false), "dist 以外（開発/Linux）では出さない");
    }

    #[test]
    fn dialog_is_never_allowed_under_cargo_test() {
        assert!(!dialog_allowed());
    }
}
