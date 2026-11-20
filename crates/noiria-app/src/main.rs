//! noiria — TypeScript で動くノベルゲームエンジン。
//! 使い方:
//!   noiria <ゲームディレクトリ>                     ウィンドウで起動
//!   noiria <ゲームディレクトリ> --autotest [秒]     ヘッドレス自動テスト（窓・音なし）
//!   noiria <ゲームディレクトリ> --autotest-out <dir> スクリーンショット出力先
//!   noiria <ゲームディレクトリ> --autotest-script <file> スクリプト駆動のヘッドレス自動テスト
//!                                                    （DSL は `autotest_script.rs`。`--autotest <秒>` 併用で仮想時間の上限）
//!   noiria <ゲームディレクトリ> --autotest-fast     実時間スリープを省いて仮想時間だけ進める
//!   noiria <ゲームディレクトリ> --save-dir <dir>    セーブ先を差し替える（環境変数 NOIRIA_SAVE_DIR でも可）
//!   noiria <ゲームディレクトリ> --engine <v8|tsint>  スクリプトエンジンを選択
//!                                                    （既定は game.json の "engine"、
//!                                                    さらに省略時は "v8"、配布ビルドは "tsint"）
//!
//! 配布ビルド（`dist` feature）: 引数省略時は exe 隣接の `game.pak`/`game/` を探す
//! （`docs/distribution.md` 参照）。

#![cfg_attr(all(windows, feature = "dist"), windows_subsystem = "windows")]
#![warn(clippy::undocumented_unsafe_blocks)]

mod autotest_script;
mod bridge;
mod clock;
#[cfg(feature = "dist")]
mod dist_logger;
mod fatal;
mod headless;
mod icon;
mod input;
mod prof;
mod script;
mod viewport;
// ウィンドウ/描画/入力は OS ネイティブ API 直叩き（`codec/mf_video.rs` と同格の
// OS 境界 API 実装）: Windows は Win32、Unix は X11 コアプロトコル自作
// （libX11/xcb 非リンク）。ヘッドレス (--autotest) はどちらにも依存しないので
// 他プラットフォームでもビルド・実行できる。
#[cfg(windows)]
mod win32;
#[cfg(windows)]
mod winbench;
#[cfg(unix)]
mod x11;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use noiria_core::assets::GameDir;
use noiria_core::audio::{self, AudioCmd, AudioShared};
use noiria_core::scene::Scene;
use tokio::sync::Notify;

use bridge::WakeSink;
use clock::Clock;
#[cfg(windows)]
use win32::WindowedParams;
#[cfg(all(unix, not(windows)))]
use x11::WindowedParams;

struct Args {
    game_path: String,
    /// 位置引数でゲームパスが明示された場合のみ true。dist ビルドの既定パス探索
    /// （exe 隣接 `game.pak`/`game/`）は false の場合だけ発動する（`dist` 未使用ビルドでは
    /// 参照されない）。
    #[cfg_attr(not(feature = "dist"), allow(dead_code))]
    game_path_explicit: bool,
    autotest: Option<Duration>,
    autotest_out: std::path::PathBuf,
    /// `--autotest-script`: 指定するとヘッドレス実行になり、固定振付の代わりにこの DSL を実行する。
    autotest_script: Option<std::path::PathBuf>,
    autotest_fast: bool,
    /// `--save-dir`: セーブ先の差し替え（自動検証が実ユーザーのセーブを汚さないため）。
    save_dir: Option<std::path::PathBuf>,
    /// `--engine` で明示指定された場合のみ Some。未指定なら game.json の
    /// `engine` フィールド（既定 "v8"、配布ビルドは "tsint"）に従う。
    engine: Option<String>,
    /// `--bench-window WxH` + `--bench-script <file>`（Windows の開発用）: 不可視ウィンドウ・無音で実ウィンドウ経路を計測する。
    bench: Option<((u32, u32), std::path::PathBuf)>,
}

fn parse_args() -> Args {
    let mut game_path = "games/sample".to_string();
    let mut game_path_explicit = false;
    let mut autotest = None;
    let mut autotest_out = std::path::PathBuf::from("target/autotest");
    let mut engine = None;
    let mut autotest_script = None;
    let mut autotest_fast = false;
    let mut save_dir = None;
    let mut bench_size = None;
    let mut bench_script = None;
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--autotest" => {
                // 次の引数が秒数なら消費する
                let secs = match argv.get(i + 1).and_then(|s| s.parse::<u64>().ok()) {
                    Some(n) => {
                        i += 1;
                        n
                    }
                    None => 25,
                };
                autotest = Some(Duration::from_secs(secs));
            }
            "--autotest-out" => {
                if let Some(dir) = argv.get(i + 1) {
                    autotest_out = dir.into();
                    i += 1;
                }
            }
            "--autotest-script" => {
                if let Some(file) = argv.get(i + 1) {
                    autotest_script = Some(file.into());
                    i += 1;
                }
            }
            "--autotest-fast" => autotest_fast = true,
            "--save-dir" => {
                if let Some(dir) = argv.get(i + 1) {
                    save_dir = Some(dir.into());
                    i += 1;
                }
            }
            "--bench-window" => {
                if let Some(v) = argv.get(i + 1) {
                    bench_size = v.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
                    i += 1;
                }
            }
            "--bench-script" => {
                if let Some(f) = argv.get(i + 1) {
                    bench_script = Some(std::path::PathBuf::from(f));
                    i += 1;
                }
            }
            "--engine" => {
                if let Some(name) = argv.get(i + 1) {
                    engine = Some(name.clone());
                    i += 1;
                }
            }
            other => {
                game_path = other.to_string();
                game_path_explicit = true;
            }
        }
        i += 1;
    }
    Args {
        game_path,
        game_path_explicit,
        autotest,
        autotest_out,
        autotest_script,
        autotest_fast,
        save_dir,
        engine,
        bench: bench_size.zip(bench_script),
    }
}

/// 配布ビルド（`dist` feature）の既定ゲームパス探索: ①exe 隣接 `game.pak` ②exe 隣接 `game/`。
/// どちらも無ければ `None`（呼び出し側が明確なエラーで停止する。開発ビルドの
/// `games/sample` フォールバックは適用しない）。
#[cfg(feature = "dist")]
fn dist_default_game_path() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let pak = dir.join("game.pak");
    if pak.is_file() {
        return Some(pak.to_string_lossy().into_owned());
    }
    let game_dir = dir.join("game");
    if game_dir.is_dir() {
        return Some(game_dir.to_string_lossy().into_owned());
    }
    None
}

/// スクリプトエンジンの起動。`spawn` のシグネチャは V8/tsint で共通なので、
/// ここで一箇所だけ分岐すればよい（呼び出し側は `--autotest`/windowed 両方）。
/// W7: engine-v8/engine-tsint feature の有無で 4 通りに分岐する。要求されたエンジンが
/// 無効ビルドの場合はパニックではなく警告 + もう一方へのフォールバックにする
/// （配布ビルドで `--engine v8` を指定してしまった場合等でも起動は継続できるように）。
#[cfg(all(feature = "engine-v8", feature = "engine-tsint"))]
fn spawn_script(name: &str, ctx: script::EngineCtx) -> std::thread::JoinHandle<()> {
    match name {
        "tsint" => {
            log::info!("スクリプトエンジン: tsint（自作 TS インタプリタ）");
            script::tsint_engine::spawn(ctx)
        }
        "v8" | "" => {
            log::info!("スクリプトエンジン: v8 (deno_core)");
            script::spawn(ctx)
        }
        other => {
            log::warn!("不明なスクリプトエンジン '{other}'。v8 にフォールバックします");
            script::spawn(ctx)
        }
    }
}

#[cfg(all(feature = "engine-v8", not(feature = "engine-tsint")))]
fn spawn_script(name: &str, ctx: script::EngineCtx) -> std::thread::JoinHandle<()> {
    if name == "tsint" {
        log::warn!(
            "tsint エンジンは無効です（engine-tsint 未使用ビルド）。v8 にフォールバックします"
        );
    } else {
        log::info!("スクリプトエンジン: v8 (deno_core)");
    }
    script::spawn(ctx)
}

#[cfg(all(feature = "engine-tsint", not(feature = "engine-v8")))]
fn spawn_script(name: &str, ctx: script::EngineCtx) -> std::thread::JoinHandle<()> {
    if name == "v8" {
        log::warn!("v8 エンジンは無効です（engine-v8 未使用ビルド）。tsint にフォールバックします");
    } else {
        log::info!("スクリプトエンジン: tsint（自作 TS インタプリタ）");
    }
    script::tsint_engine::spawn(ctx)
}

#[cfg(not(any(feature = "engine-v8", feature = "engine-tsint")))]
fn spawn_script(_name: &str, _ctx: script::EngineCtx) -> std::thread::JoinHandle<()> {
    compile_error!(
        "少なくとも一方の engine feature (engine-v8 / engine-tsint) を有効にしてください"
    );
}

/// `spawn_script` と同じフォールバックルールで、実際に起動されるエンジンが
/// tsint かどうかを判定する。ヘッドレスの tick ドレインバリア
/// （`headless::HeadlessParams::tick_barrier`）を tsint 実行時のみ有効にするために使う
/// （V8 は既に完全決定論のため対象外。`bridge::TickSync` のドキュメント参照）。
#[cfg(all(feature = "engine-v8", feature = "engine-tsint"))]
fn resolved_engine_is_tsint(name: &str) -> bool {
    name == "tsint"
}

#[cfg(all(feature = "engine-v8", not(feature = "engine-tsint")))]
fn resolved_engine_is_tsint(_name: &str) -> bool {
    false
}

#[cfg(all(feature = "engine-tsint", not(feature = "engine-v8")))]
fn resolved_engine_is_tsint(_name: &str) -> bool {
    true
}

#[cfg(not(any(feature = "engine-v8", feature = "engine-tsint")))]
fn resolved_engine_is_tsint(_name: &str) -> bool {
    false
}

fn main() -> std::process::ExitCode {
    // ヘッドレス（--autotest*）ではダイアログを絶対に出さない。引数の本解析より前に
    // panic フックが動きうるので、ここで先に判定する。
    let headless = std::env::args().any(|a| a == "--autotest" || a == "--autotest-script");
    fatal::set_headless(headless);
    fatal::install_panic_hook();
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            fatal::report("Noiria - エラー", &format!("{e:#}"));
            std::process::ExitCode::FAILURE
        }
    }
}

/// `dist` でも `dev-log`（env_logger）でもない構成（engine-tsint 単独の非 dist ビルド）で
/// ログを消さないための最小ロガー。レベルは `NOIRIA_LOG`（既定 info）。
#[cfg(all(not(feature = "dist"), not(feature = "dev-log")))]
mod stderr_logger {
    use log::{LevelFilter, Log, Metadata, Record};

    struct StderrLogger(LevelFilter);

    impl Log for StderrLogger {
        fn enabled(&self, metadata: &Metadata) -> bool {
            metadata.level() <= self.0
        }
        fn log(&self, record: &Record) {
            if self.enabled(record.metadata()) {
                eprintln!("[{} {}] {}", record.level(), record.target(), record.args());
            }
        }
        fn flush(&self) {}
    }

    pub fn init() {
        let level = std::env::var("NOIRIA_LOG")
            .ok()
            .and_then(|s| s.parse::<LevelFilter>().ok())
            .unwrap_or(LevelFilter::Info);
        if log::set_logger(Box::leak(Box::new(StderrLogger(level)))).is_ok() {
            log::set_max_level(level);
        }
    }
}

fn run() -> anyhow::Result<()> {
    #[cfg(feature = "dist")]
    let dist_logger = dist_logger::init();
    #[cfg(all(not(feature = "dist"), feature = "dev-log"))]
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    #[cfg(all(not(feature = "dist"), not(feature = "dev-log")))]
    stderr_logger::init();

    #[cfg_attr(not(feature = "dist"), allow(unused_mut))]
    let mut args = parse_args();
    #[cfg(feature = "dist")]
    if !args.game_path_explicit {
        args.game_path = dist_default_game_path().ok_or_else(|| {
            anyhow::anyhow!(
                "配布ゲームが見つかりません: exe と同じフォルダに game.pak または game フォルダを置いてください"
            )
        })?;
    }
    if let Some(dir) = &args.save_dir {
        noiria_core::savedata::set_save_dir_override(dir.clone());
    }
    let (game_dir, config) = GameDir::open(&args.game_path)?;
    #[cfg(feature = "dist")]
    dist_logger.switch_to_game_log(&config.id);
    log::info!(
        "ゲーム: {} ({}x{})",
        config.title,
        config.width,
        config.height
    );
    let engine_name = args.engine.clone().unwrap_or_else(|| config.engine.clone());
    // ウィンドウ/タスクバーアイコン: ゲーム側 icon.png（無ければエンジン既定）。
    // ヘッドレス (--autotest) では未使用だが、単純さのため常に計算する。
    let icon_png = game_dir.read_bytes("icon.png").ok();
    let app_icon = icon::AppIcon::from_optional_png(icon_png.as_deref());

    let scene = Arc::new(Mutex::new(Scene::new(config.width, config.height)));
    let start = Instant::now();
    let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel();
    let trans_notify = Arc::new(Notify::new());
    let (audio_tx, audio_rx) = crossbeam_channel::unbounded();
    let audio_shared = Arc::new(AudioShared::default());

    if args.autotest.is_some() || args.autotest_script.is_some() {
        // スクリプトモードの `--autotest <秒>` は仮想時間の上限（無指定なら 1 時間）。
        let duration = args.autotest.unwrap_or(Duration::from_secs(3600));
        let script = match &args.autotest_script {
            Some(path) => {
                let text = std::fs::read_to_string(path).with_context(|| {
                    format!("autotest スクリプトを読めません: {}", path.display())
                })?;
                Some(
                    autotest_script::parse(&text)
                        .with_context(|| format!("{}", path.display()))?,
                )
            }
            None => None,
        };
        // ヘッドレス: オーディオスレッドは起動しない（無音）。受信口は捨てる。
        drop(audio_rx);
        // ヘッドレス専用の仮想クロック。headless::run_headless のフレームループだけが
        // 進め、スクリプトスレッド（EngineState）はアニメーション開始時刻の記録に
        // 同じインスタンスを読む（`clock.rs` のドキュメント参照）。
        let clock = Clock::new_virtual(start);
        // headless 専用のロックステップ同期カウンタ（`bridge::InputSync` 参照）。
        // headless::run_headless と script スレッドの両方へ同じインスタンスを共有する
        // （`clock` と同じ配線パターン）。
        let input_sync = bridge::InputSync::new();
        // tick ドレインバリア用カウンタ（`bridge::TickSync` 参照）。tsint 実行時のみ
        // headless 側が実際に待つ（`tick_barrier`）。
        let tick_sync = bridge::TickSync::new();
        let tick_barrier = resolved_engine_is_tsint(&engine_name);
        // V8 専用: 仮想 `op_wait` の in-flight カウンタ（`bridge::PendingWaits` 参照）。
        // headless 側がこれを読んで V8 の tick ドレインバリアの要否を判定する
        // （`tick_sync`/`clock` と同じ配線パターン）。tsint はこれに触れないため
        // 常に 0 のまま。
        let pending_waits = bridge::PendingWaits::new();
        let (user_tx, user_rx) = crossbeam_channel::unbounded();
        let _script_thread = spawn_script(
            &engine_name,
            script::EngineCtx {
                scene: scene.clone(),
                proxy: WakeSink::Channel(user_tx),
                input_rx,
                trans_notify: trans_notify.clone(),
                audio_tx,
                audio_shared,
                game_dir,
                config: config.clone(),
                start,
                clock: clock.clone(),
                input_sync: input_sync.clone(),
                tick_sync: tick_sync.clone(),
                pending_waits: pending_waits.clone(),
                op_log: None,
            },
        );
        return headless::run_headless(headless::HeadlessParams {
            scene,
            input_tx,
            trans_notify,
            user_rx,
            start,
            clock,
            input_sync,
            tick_sync,
            tick_barrier,
            pending_waits,
            duration,
            out_dir: args.autotest_out,
            width: config.width,
            height: config.height,
            script,
            fast: args.autotest_fast,
        });
    }

    // 計測モードは無音: オーディオスレッドを起動せず、受信口は捨てる（送信は黙って失敗する）。
    let _audio_thread = if args.bench.is_some() {
        drop(audio_rx);
        None
    } else {
        Some(audio::spawn(audio_rx, audio_shared.clone(), game_dir.clone()))
    };

    #[cfg(windows)]
    {
        use std::sync::atomic::AtomicIsize;

        let (user_tx, user_rx) = crossbeam_channel::unbounded();
        let user_tx_bench = user_tx.clone();
        let hwnd_slot = Arc::new(AtomicIsize::new(0));
        let wake_posted = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let _script_thread = spawn_script(
            &engine_name,
            script::EngineCtx {
                scene: scene.clone(),
                proxy: WakeSink::Win32 {
                    tx: user_tx,
                    hwnd: hwnd_slot.clone(),
                    posted: wake_posted.clone(),
                },
                input_rx,
                trans_notify: trans_notify.clone(),
                audio_tx: audio_tx.clone(),
                audio_shared,
                game_dir,
                config: config.clone(),
                start,
                clock: Clock::Real,
                input_sync: bridge::InputSync::new(),
                tick_sync: bridge::TickSync::new(),
                pending_waits: bridge::PendingWaits::new(),
                op_log: None,
            },
        );

        let mut visible = true;
        if let Some((size, file)) = &args.bench {
            let text = std::fs::read_to_string(file)
                .with_context(|| format!("bench スクリプトを読めません: {}", file.display()))?;
            let steps = autotest_script::parse(&text).with_context(|| format!("{}", file.display()))?;
            visible = false;
            winbench::spawn(hwnd_slot.clone(), user_tx_bench, start, *size, config.width, steps);
        }
        win32::run_windowed(WindowedParams {
            title: config.title.clone(),
            game_w: config.width,
            game_h: config.height,
            scene,
            input_tx,
            trans_notify,
            start,
            user_rx,
            hwnd_slot,
            wake_posted,
            icon: app_icon,
            visible,
        })?;
    }
    #[cfg(all(unix, not(windows)))]
    {
        let (user_tx, user_rx) = crossbeam_channel::unbounded();

        let _script_thread = spawn_script(
            &engine_name,
            script::EngineCtx {
                scene: scene.clone(),
                proxy: WakeSink::Channel(user_tx),
                input_rx,
                trans_notify: trans_notify.clone(),
                audio_tx: audio_tx.clone(),
                audio_shared,
                game_dir,
                config: config.clone(),
                start,
                clock: Clock::Real,
                input_sync: bridge::InputSync::new(),
                tick_sync: bridge::TickSync::new(),
                pending_waits: bridge::PendingWaits::new(),
                op_log: None,
            },
        );

        x11::run_windowed(WindowedParams {
            title: config.title.clone(),
            game_w: config.width,
            game_h: config.height,
            scene,
            input_tx,
            trans_notify,
            start,
            user_rx,
            icon: app_icon,
            visible: true,
        })?;
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (game_dir, input_rx, trans_notify, scene, input_tx, app_icon);
        anyhow::bail!(
            "ウィンドウ表示は Windows (Win32) / Unix (X11 直叩き) のみ対応です\
             （--autotest はどの OS でも動作します）"
        );
    }

    let _ = audio_tx.send(AudioCmd::Quit);
    match fatal::take_script_error() {
        Some(msg) => anyhow::bail!("ゲームスクリプトがエラーで終了しました: {msg}"),
        None => Ok(()),
    }
}
