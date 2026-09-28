use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct TempVault {
    pub root: PathBuf,
}

impl TempVault {
    pub fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("pkmagent-{}-{n}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    pub fn inbox(&self) -> PathBuf {
        self.root.join("inbox")
    }
}

impl Drop for TempVault {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
