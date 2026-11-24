//! 配布ビルド専用の自前ファイルロガー（`dist` feature、`env_logger` の代替）。
//! `docs/implementation-notes.md` 2026-07-12 配布対応 W7 参照。
//!
//! `game_id` が確定する前（`GameDir::open` 自体が失敗した場合など）のログは exe 隣接
//! `noiria-startup.log` に書く。`GameDir::open` 成功後は [`FileLogger::switch_to_game_log`]
//! でセーブディレクトリ配下 `noiria.log` に切り替える（以後のログはそちらに集約され、
//! 通常起動では起動直後にほぼ全ログがこちらへ移る）。どちらのファイルも開けない場合は
//! stderr にフォールバックする（`windows_subsystem="windows"` 下では誰にも見えないが、
//! ファイルすら書けない異常環境でログを完全に失うよりはましという判断）。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use log::{LevelFilter, Log, Metadata, Record};

use crate::bridge::LockExt;

static LOG_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 現在ログの書き込み先（致命エラーダイアログで案内する）。
pub(crate) fn log_path() -> Option<PathBuf> {
    LOG_PATH.lock_unpoisoned().clone()
}

pub(crate) struct FileLogger {
    file: Mutex<Option<File>>,
    level: LevelFilter,
}

fn open_append(path: &std::path::Path) -> Option<File> {
    let f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    *LOG_PATH.lock_unpoisoned() = Some(path.to_path_buf());
    Some(f)
}

fn exe_adjacent_log_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("noiria-startup.log"))
}

impl FileLogger {
    fn new(level: LevelFilter) -> Self {
        let file = exe_adjacent_log_path().and_then(|p| open_append(&p));
        Self {
            file: Mutex::new(file),
            level,
        }
    }

    /// `game_id` が確定した後に呼ぶ: 以後のログをセーブディレクトリの `noiria.log` に切り替える。
    /// 切り替えに失敗した場合は exe 隣接ログのまま続行する（呼び出し元は結果を無視してよい）。
    pub(crate) fn switch_to_game_log(&self, game_id: &str) {
        let Ok(dir) = noiria_core::savedata::save_dir(game_id) else {
            return;
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        if let Some(f) = open_append(&dir.join("noiria.log")) {
            *self.file.lock_unpoisoned() = Some(f);
        }
    }
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // `NOIRIA_PROF` の計測行（target = noiria::prof、info）は既定の warn でも出す。
        metadata.level() <= self.level || metadata.target() == "noiria::prof"
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let line = format!(
            "[{}.{:03}] {:5} {}: {}\n",
            ts.as_secs(),
            ts.subsec_millis(),
            record.level(),
            record.target(),
            record.args()
        );
        let mut guard = self.file.lock_unpoisoned();
        match guard.as_mut() {
            Some(f) => {
                let _ = f.write_all(line.as_bytes());
                let _ = f.flush();
            }
            None => {
                drop(guard);
                eprint!("{line}");
            }
        }
    }

    fn flush(&self) {
        if let Some(f) = self.file.lock_unpoisoned().as_mut() {
            let _ = f.flush();
        }
    }
}

/// ロガーを初期化してグローバル登録する。レベルは環境変数 `NOIRIA_LOG`
/// （`error`/`warn`/`info`/`debug`/`trace`、既定 `warn`）。
pub(crate) fn init() -> &'static FileLogger {
    let level = std::env::var("NOIRIA_LOG")
        .ok()
        .and_then(|s| s.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::Warn);
    let logger: &'static FileLogger = Box::leak(Box::new(FileLogger::new(level)));
    log::set_logger(logger).expect("ロガーは一度しか初期化できません");
    log::set_max_level(if crate::prof::enabled() {
        level.max(LevelFilter::Info)
    } else {
        level
    });
    logger
}
