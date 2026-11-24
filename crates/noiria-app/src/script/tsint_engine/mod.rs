//! tsint（完全自作 TypeScript インタプリタ、`../../../tsint`）スクリプトエンジン。
//!
//! `script::spawn`（V8/deno_core 版）と同じ `EngineCtx` を受け取り、同じ契約
//! （スクリプト終了時に必ず `WakeSink` へ Quit を送る）で動く、もうひとつの
//! スクリプトエンジン。ops の本体ロジックは `engine_state::EngineState` を
//! 両エンジンで共用する。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::AtomicU64;
use std::thread::JoinHandle;

use noiria_core::assets::GameDir;
use tsint::module::{ModuleHost, ModuleOutcome};
use tsint::Interpreter;

use crate::bridge::UserEvent;
use crate::script::engine_state::EngineState;
use crate::script::runtime_modules::RUNTIME_MODULES;
use crate::script::EngineCtx;

mod convert;
mod event_loop;
mod ops;
#[cfg(test)]
mod tests;

use convert::debug_value;
use event_loop::{drive, Intake, PendingOps};
use ops::install_ops;

// ModuleHost: `noiria:` は埋め込み TS ライブラリ（V8 版と同じテーブルを共用）、
// それ以外はゲームディレクトリ相対のファイル（相対 import にも対応）。
// =====================================================================

struct NoiriaTsHost {
    game_dir: GameDir,
}

/// `referrer_rel`（ゲームルート相対、`/` 区切り）を基準に `specifier` を
/// ゲームルート相対パスへ正規化する。`..` は素朴に 1 段階分だけ親を辿る
/// （ルートより上には出られない: 既に正規化済みの `base` は `..` を含まない
/// ため、空の状態で `pop()` しても単に無視されるだけでルート外へは脱出しない）。
fn normalize_game_rel(referrer_rel: &str, specifier: &str) -> String {
    let mut base: Vec<&str> = if specifier.starts_with("./") || specifier.starts_with("../") {
        match referrer_rel.rfind('/') {
            Some(i) => referrer_rel[..i]
                .split('/')
                .filter(|s| !s.is_empty())
                .collect(),
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };
    for comp in specifier.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                base.pop();
            }
            other => base.push(other),
        }
    }
    base.join("/")
}

impl ModuleHost for NoiriaTsHost {
    fn resolve(&self, specifier: &str, referrer: &str) -> Result<String, String> {
        if specifier.starts_with("noiria:") {
            return Ok(specifier.to_string());
        }
        let ref_rel = if referrer == "<entry>" {
            ""
        } else {
            referrer.strip_prefix("game:").unwrap_or(referrer)
        };
        Ok(format!("game:{}", normalize_game_rel(ref_rel, specifier)))
    }

    fn load(&self, resolved: &str) -> Result<String, String> {
        if resolved.starts_with("noiria:") {
            RUNTIME_MODULES
                .iter()
                .find(|(k, _)| *k == resolved)
                .map(|(_, v)| v.to_string())
                .ok_or_else(|| format!("組み込みモジュールがありません: {resolved}"))
        } else if let Some(rel) = resolved.strip_prefix("game:") {
            // パスジェイルは `GameDir::resolve`（`read_text` 内部で使用）に委譲する。
            self.game_dir.read_text(rel).map_err(|e| e.to_string())
        } else {
            Err(format!("未対応のモジュール指定子です: {resolved}"))
        }
    }
}


// =====================================================================
// スクリプトスレッド本体
// =====================================================================

/// スクリプトスレッドを起動する。スクリプト終了（正常/異常とも）でメインループへ Quit を送る
/// （V8 版 `script::spawn` と同じ契約）。
pub fn spawn(ctx: EngineCtx) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("noiria-script-tsint".into())
        .spawn(move || {
            let wake = ctx.proxy.clone();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_script(ctx))) {
                Ok(Ok(())) => {}
                Ok(Err(e)) => crate::fatal::record_script_error(format!("{e} (tsint)")),
                Err(p) => crate::fatal::record_script_error(crate::fatal::panic_message(&*p)),
            }
            wake.send(UserEvent::Quit);
        })
        .expect("スクリプトスレッド(tsint)の起動に失敗")
}

/// エンジン本体。tokio を使わないので同期関数のまま完結する。
fn run_script(ctx: EngineCtx) -> Result<(), String> {
    run_script_counted(ctx, None)
}

/// `run_script` の計測つき版。`wakes` があれば、`drive` が 1 周するたびに加算する（待機中の起床回数の測定用）。
fn run_script_counted(ctx: EngineCtx, wakes: Option<&AtomicU64>) -> Result<(), String> {
    let main_rel = ctx.config.main.clone();
    let host = NoiriaTsHost {
        game_dir: ctx.game_dir.clone(),
    };

    let (engine_state, input_rx) = EngineState::from_ctx(ctx);
    let es = Rc::new(RefCell::new(engine_state));
    let pending = Rc::new(RefCell::new(PendingOps::default()));

    let mut interp = Interpreter::new();
    interp.set_now_fn(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    });
    interp.set_console_hook(|s| log::info!("[tsint console] {s}"));
    interp.set_unhandled_rejection_hook(|v| {
        log::warn!("[tsint] 未処理の Promise reject: {}", debug_value(v))
    });

    // CJK フォントの起動時チェック（失敗しても続行、ログのみ。V8 版と同じ挙動）。
    {
        let style = noiria_core::text::TextStyle::default();
        if let Err(e) = es.borrow_mut().texts.engine.assert_cjk(&style) {
            log::warn!("{e:#}");
        }
    }

    install_ops(&mut interp, &es, &pending);

    let outcome = interp
        .run_module_graph(&main_rel, &host)
        .map_err(|e| e.to_string())?;
    match outcome {
        ModuleOutcome::Settled(_) => {
            log::info!("tsint: メインモジュールの評価が完了しました（await 待ちなし）");
            Ok(())
        }
        ModuleOutcome::Pending => {
            let real = es.borrow().clock.is_real();
            let trans_notify = es.borrow().trans_notify.clone();
            let intake = if real {
                Intake::forwarded(input_rx, trans_notify)
            } else {
                Intake::Direct(input_rx)
            };
            drive(&mut interp, &es, &pending, intake, wakes)
        }
    }
}

