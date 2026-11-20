//! 単体テスト/統合テスト共有の一時ディレクトリ（RAII）。panic しても `Drop` で掃除される。
//! 統合テスト（`tests/`）からも使うため `cfg(test)` ではなく通常の公開モジュールにしている。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

pub struct TestRoot(PathBuf);

impl TestRoot {
    pub fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "noiria_{label}_{}_{nanos}_{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// `rel` へ内容を書く（親ディレクトリも作る）。
    pub fn write(&self, rel: &str, content: &[u8]) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        p
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
