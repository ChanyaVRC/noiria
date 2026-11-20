//! `NOIRIA_PROF=1` の実機計測ログ。描画ループが 1 フレーム描くたびに 1 行を `noiria.log`（配布ビルド）/ 標準エラー
//! （開発ビルド）へ出す。環境変数が無いときのコストは `enabled()` の判定（初回のみ）だけ。
//!
//! 形式: `[PROF] t=<起動からの ms> compose=<ms> convert=<ms> blit=<ms> lock_wait=<ms> held=<ms> layers=<可視レイヤ数>
//! anim=<0/1> wakes=<前回から受けた再描画要求数> client=<W>x<H> since_input=<最後のクリック/キーから ms>`。`held` は合成と転送の間シーンのロックを握っていた時間
//! （この間スクリプト側の ops は待たされる）。

// 計測の呼び出しは Win32 の描画ループ側にしか配線されていない（X11 は未配線）。
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// `NOIRIA_PROF=2` なら、可視でないウィンドウでも拡大転送の実コストが出るよう、同じ転送をメモリ DC へも行って測る
/// （不可視ウィンドウへの `StretchDIBits` はクリップされて実際には何もしない）。
pub fn deep() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("NOIRIA_PROF").is_some_and(|v| v == "2"))
}

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("NOIRIA_PROF").is_some_and(|v| v != "0"))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[allow(clippy::too_many_arguments)]
pub fn frame(
    start: Instant,
    compose: Duration,
    convert: Duration,
    scale: Duration,
    blit: Duration,
    memblit: Duration,
    lock_wait: Duration,
    held: Duration,
    layers: usize,
    animating: bool,
    wakes: u32,
    client: (u32, u32),
    since_input: Option<Duration>,
) {
    log::info!(
        "[PROF] t={:.1} compose={:.2} convert={:.2} scale={:.2} blit={:.2} memblit={:.2} lock_wait={:.2} held={:.2} layers={} anim={} wakes={} client={}x{} since_input={:.1}",
        ms(start.elapsed()),
        ms(compose),
        ms(convert),
        ms(scale),
        ms(blit),
        ms(memblit),
        ms(lock_wait),
        ms(held),
        layers,
        animating as u8,
        wakes,
        client.0,
        client.1,
        since_input.map_or(-1.0, ms)
    );
}

/// 入力などの目印（計測スクリプトの `mark` 用）。
pub fn mark(start: Instant, label: &str) {
    log::info!("[PROF] t={:.1} mark={}", ms(start.elapsed()), label);
}
