use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

struct Logger {
    file: Mutex<File>,
    invocation_id: String,
}

static LOGGER: OnceLock<Option<Logger>> = OnceLock::new();
static STDERR_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enable_stderr(cli_enabled: bool) {
    STDERR_ENABLED.store(cli_enabled || env_stderr_enabled(), Ordering::Relaxed);
}

pub fn stderr_enabled() -> bool {
    STDERR_ENABLED.load(Ordering::Relaxed)
}

fn env_stderr_enabled() -> bool {
    match std::env::var("WIKI_PERF") {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

pub struct Span {
    label: String,
    start: Option<Instant>,
}

impl Span {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            start: stderr_enabled().then(Instant::now),
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let Some(start) = self.start else {
            return;
        };
        eprintln!(
            "wiki perf: {} {:.3} ms",
            self.label,
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
}

pub fn span_for_command(command_name: &str) -> Span {
    Span::new(format!("command.{command_name}"))
}

pub fn init(repo_root: &Path, command_name: &str, json_output: bool) {
    let _ = LOGGER.get_or_init(|| {
        let path = log_path(repo_root)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        let logger = Logger {
            file: Mutex::new(file),
            invocation_id: format!(
                "{}-{}",
                std::process::id(),
                unix_time_now_ms().unwrap_or_default()
            ),
        };
        write_event(
            &logger,
            "command_start",
            0.0,
            "ok",
            json!({
                "command": command_name,
                "json_output": json_output,
                "repo_root": repo_root.display().to_string(),
            }),
        );
        Some(logger)
    });
}

pub fn finish(command_name: &str, exit_code: i32, total_ms: f64, status: &str) {
    log_event(
        "command_finish",
        total_ms,
        status,
        json!({
            "command": command_name,
            "exit_code": exit_code,
        }),
    );
}

pub fn log_event(name: &str, duration_ms: f64, status: &str, meta: Value) {
    if let Some(Some(logger)) = LOGGER.get() {
        write_event(logger, name, duration_ms, status, meta);
    }
    if stderr_enabled() && name != "command_start" && name != "command_finish" {
        eprintln!("wiki perf: {name} {duration_ms:.3} ms");
    }
}

pub fn scope_result<T, E>(
    name: &str,
    meta: Value,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let start = Instant::now();
    let result = f();
    let status = if result.is_ok() { "ok" } else { "error" };
    log_event(name, start.elapsed().as_secs_f64() * 1000.0, status, meta);
    result
}

pub async fn scope_async_result<T, E, F>(name: &str, meta: Value, future: F) -> Result<T, E>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    let start = Instant::now();
    let result = future.await;
    let status = if result.is_ok() { "ok" } else { "error" };
    log_event(name, start.elapsed().as_secs_f64() * 1000.0, status, meta);
    result
}

fn log_path(repo_root: &Path) -> Option<PathBuf> {
    let wiki_dir_name = std::env::var("WIKI_DIR").unwrap_or_else(|_| "wiki".to_string());
    let wiki_dir_path = PathBuf::from(wiki_dir_name);
    let wiki_dir = if wiki_dir_path.is_absolute() {
        wiki_dir_path
    } else {
        repo_root.join(wiki_dir_path)
    };
    fs::create_dir_all(&wiki_dir).ok()?;
    Some(wiki_dir.join("wiki.log"))
}

fn write_event(logger: &Logger, name: &str, duration_ms: f64, status: &str, meta: Value) {
    let timestamp_ms = unix_time_now_ms().unwrap_or_default();
    let payload = json!({
        "timestamp_ms": timestamp_ms,
        "invocation_id": logger.invocation_id,
        "pid": std::process::id(),
        "event": name,
        "duration_ms": duration_ms,
        "status": status,
        "meta": meta,
    });

    if let Ok(mut file) = logger.file.lock() {
        let _ = writeln!(file, "{payload}");
    }
}

fn unix_time_now_ms() -> Option<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis())
}
