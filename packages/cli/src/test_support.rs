use std::sync::{Mutex, MutexGuard, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub struct WikiDirGuard {
    _lock: MutexGuard<'static, ()>,
    previous: Option<String>,
}

pub fn set_wiki_dir(value: &str) -> WikiDirGuard {
    let lock = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var("WIKI_DIR").ok();
    unsafe {
        std::env::set_var("WIKI_DIR", value);
    }
    WikiDirGuard {
        _lock: lock,
        previous,
    }
}

impl Drop for WikiDirGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(previous) => unsafe {
                std::env::set_var("WIKI_DIR", previous);
            },
            None => unsafe {
                std::env::remove_var("WIKI_DIR");
            },
        }
    }
}
