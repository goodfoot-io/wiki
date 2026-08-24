//! Countable store diagnostic events (plan D11).
//!
//! [`STORE_EVENTS_DDL`](crate::cache::schema::STORE_EVENTS_DDL) materializes
//! the `store_events(stamp, event)` append ledger on every open; this module
//! is its write and read surface. Two contracts shape it:
//!
//! * [`record`] is **best-effort and infallible to its caller**: any SQLite
//!   failure is swallowed, so a diagnostics write can never turn a healthy
//!   cache operation into a fault (the cache's fail-open posture). The
//!   ledger prunes to the newest ~[`KEEP_NEWEST`] rows periodically — every
//!   [`PRUNE_INTERVAL`]-th insert — bounding growth without a background
//!   timer.
//!
//! * [`publish_counts`] aggregates per-event totals into the perf JSON-lines
//!   channel ([`crate::perf`]): the run's aggregated `anchor_cache` record
//!   carries them under `meta.diagnostics`, omitted when no store
//!   connection published on the emitting path. The stderr `--perf` echo is
//!   deliberately untouched (plan D11).
//!
//! Cross-tier infrastructure by design: quarantine/rebuild/skew-repair come
//! from the shared open path, generation-side GC events may come from any
//! tier's connection ([`record]` takes any connection to the shared file),
//! and tier-scoped invalidation never drops the table.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

/// Rows retained by the periodic prune (plan D11: newest ~1000).
const KEEP_NEWEST: i64 = 1000;

/// Prune fires on every Nth successful insert, amortizing the delete scan.
const PRUNE_INTERVAL: i64 = 128;

/// Record one diagnostic event, best-effort. Never fails, never panics on
/// SQLite trouble: a missing or damaged ledger silently drops the row.
pub fn record(conn: &Connection, event: &str) {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let inserted = conn.execute(
        "INSERT INTO store_events (stamp, event) VALUES (?1, ?2)",
        params![stamp, event],
    );
    if inserted.is_ok() && conn.last_insert_rowid() % PRUNE_INTERVAL == 0 {
        let _ = conn.execute(
            "DELETE FROM store_events WHERE rowid NOT IN (
                 SELECT rowid FROM store_events ORDER BY rowid DESC LIMIT ?1
             )",
            params![KEEP_NEWEST],
        );
    }
}

/// Aggregate per-event counts from the ledger into the perf JSON-lines
/// channel. Best-effort like [`record`]: an unreadable ledger simply
/// publishes nothing. An empty ledger clears any earlier snapshot, so a
/// clean run's payload omits the field entirely.
pub fn publish_counts(conn: &Connection) {
    let counts = counts(conn);
    crate::perf::set_diagnostic_counts(counts);
}

/// Per-event row counts, ordered by event name for a stable payload.
fn counts(conn: &Connection) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    let Ok(mut stmt) = conn.prepare("SELECT event, count(*) FROM store_events GROUP BY event")
    else {
        return counts;
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
    }) else {
        return counts;
    };
    for row in rows.flatten() {
        counts.insert(row.0, row.1);
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::schema::STORE_EVENTS_DDL;

    #[test]
    fn record_is_infallible_without_the_ledger_table() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = Connection::open(dir.path().join("x.sqlite")).expect("open");
        // No DDL ran: store_events does not exist. Must not panic.
        record(&conn, "quarantine_performed");
    }

    #[test]
    fn prune_bounds_the_ledger_near_keep_newest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = Connection::open(dir.path().join("x.sqlite")).expect("open");
        conn.execute_batch(STORE_EVENTS_DDL).expect("ddl");
        for i in 0..(KEEP_NEWEST * 3 + PRUNE_INTERVAL) {
            record(&conn, if i % 2 == 0 { "even" } else { "odd" });
        }
        let total: i64 = conn
            .query_row("SELECT count(*) FROM store_events", [], |r| r.get(0))
            .expect("count");
        assert!(
            total >= KEEP_NEWEST && total < KEEP_NEWEST + PRUNE_INTERVAL,
            "ledger must stay within [KEEP_NEWEST, KEEP_NEWEST + PRUNE_INTERVAL), got {total}"
        );
    }

    #[test]
    fn counts_aggregate_per_event_and_stay_sorted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = Connection::open(dir.path().join("x.sqlite")).expect("open");
        conn.execute_batch(STORE_EVENTS_DDL).expect("ddl");
        record(&conn, "rebuild_completed");
        record(&conn, "quarantine_performed");
        record(&conn, "quarantine_performed");
        let counts = counts(&conn);
        let flat: Vec<(&str, u64)> = counts.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        assert_eq!(
            flat,
            vec![("quarantine_performed", 2), ("rebuild_completed", 1)],
            "BTreeMap ordering keeps the payload stable"
        );
    }
}
