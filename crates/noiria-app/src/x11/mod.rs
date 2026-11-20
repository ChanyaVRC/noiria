//! X11 コアプロトコル直叩きのウィンドウ/描画/入力（Linux/Unix 専用）。
//! libX11/xcb には一切リンクせず、Unix ドメインソケット上で X11 のバイナリ
//! プロトコルを自前で実装する（`win32.rs` の Win32 直叩きと同じ思想）。
//!
//! - `conn`: DISPLAY 解析・ソケット接続・Xauthority・セットアップハンドシェイク。
//! - `proto`: リクエスト直列化・リプライ/イベントのパース・sequence 照合。
//! - `keymap`: GetKeyboardMapping の結果を keysym テーブル化し、win32.rs と
//!   同じキー集合へマッピングする。
//! - `window`: `run_windowed` 本体（win32.rs と同型のメッセージループ）。

pub mod conn;
pub mod keymap;
pub mod proto;
pub mod window;

pub use window::{run_windowed, WindowedParams};
