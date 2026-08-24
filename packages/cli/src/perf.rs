use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

struct Logger {
    file: Mutex<File>,
    invocation_id: String,
}

static LOGGER: OnceLock<Option<Logger>> = OnceLock::new();
static STDERR_ENABLED: AtomicBool = AtomicBool::new(false);

/// Per-run anchor-cache tallies (plan decision 7): the tiers record hits,
/// misses, bypasses, and summed git-leg durations here, and the run emits
/// them as a single aggregated `anchor_cache` event — per-link or per-page
/// events would flood `wiki.log` (a 10k-link corpus → 10k+ lines per run).
struct AnchorCacheCounters {
    hits: AtomicU64,
    misses: AtomicU64,
    bypasses: AtomicU64,
    fingerprint_ns: AtomicU64,
    walk_ns: AtomicU64,
}

static ANCHOR_CACHE: AnchorCacheCounters = AnchorCacheCounters {
    hits: AtomicU64::new(0),
    misses: AtomicU64::new(0),
    bypasses: AtomicU64::new(0),
    fingerprint_ns: AtomicU64::new(0),
    walk_ns: AtomicU64::new(0),
};

pub fn anchor_cache_hit() {
    ANCHOR_CACHE.hits.fetch_add(1, Ordering::Relaxed);
}

pub fn anchor_cache_miss() {
    ANCHOR_CACHE.misses.fetch_add(1, Ordering::Relaxed);
}

pub fn anchor_cache_bypass() {
    ANCHOR_CACHE.bypasses.fetch_add(1, Ordering::Relaxed);
}

/// Sum a tier's git-leg duration (the memoized cost on the miss path). The
/// `fingerprint`/`walk` split preserves the per-tier economy decomposition
/// the benchmark and the warm-run economy check rely on.
pub fn anchor_cache_add_leg(name: &str, ns: u64) {
    match name {
        "fingerprint" => ANCHOR_CACHE.fingerprint_ns.fetch_add(ns, Ordering::Relaxed),
        "walk" => ANCHOR_CACHE.walk_ns.fetch_add(ns, Ordering::Relaxed),
        _ => 0,
    };
}

/// Per-run store diagnostic tallies (plan D11): the merged store publishes
/// its `store_events` per-event counts when a connection is available on the
/// run's path, and the aggregated `anchor_cache` record carries them under
/// `meta.diagnostics`. `None` (kill switch, held lock, disabled cache) means
/// the field is omitted entirely.
static DIAGNOSTICS: Mutex<Option<BTreeMap<String, u64>>> = Mutex::new(None);

/// Publish one run's store_events aggregation, replacing any earlier
/// snapshot. An empty map clears the slot so a clean run omits the field.
pub fn set_diagnostic_counts(counts: BTreeMap<String, u64>) {
    if let Ok(mut slot) = DIAGNOSTICS.lock() {
        *slot = (!counts.is_empty()).then_some(counts);
    }
}

fn diagnostic_snapshot() -> Option<BTreeMap<String, u64>> {
    let guard = DIAGNOSTICS.lock().ok()?;
    guard.clone()
}

/// Emit the run's one aggregated `anchor_cache` event (plan decision 7).
/// Called once per `wiki check` invocation, after the run body, on every
/// path — early exits included — so a warm run reports zero legs rather
/// than nothing. A no-op before `perf::init` resolves the log file.
pub fn emit_anchor_cache_event() {
    let mut meta = json!({
        "hits": ANCHOR_CACHE.hits.load(Ordering::Relaxed),
        "misses": ANCHOR_CACHE.misses.load(Ordering::Relaxed),
        "bypasses": ANCHOR_CACHE.bypasses.load(Ordering::Relaxed),
        "fingerprint_ms": ANCHOR_CACHE.fingerprint_ns.load(Ordering::Relaxed) as f64 / 1e6,
        "walk_ms": ANCHOR_CACHE.walk_ns.load(Ordering::Relaxed) as f64 / 1e6,
    });
    // Countable store diagnostics (plan D11) ride in the event's payload;
    // the `--perf` stderr echo prints only name+duration by design and is
    // untouched by this.
    if let Some(counts) = diagnostic_snapshot() {
        meta["diagnostics"] = json!(counts);
    }
    log_event("anchor_cache", 0.0, "ok", meta);
}

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

fn log_path(repo_root: &Path) -> Option<PathBuf> {
    fs::create_dir_all(repo_root).ok()?;
    Some(repo_root.join("wiki.log"))
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
