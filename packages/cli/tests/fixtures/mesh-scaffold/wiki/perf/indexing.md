---
title: Incremental indexing
summary: How the wiki indexer detects and applies changes.
---

See [bootstrap](src/index.rs#L1-L5) for the entry point.

# Incremental indexing

## Sync detection

The WikiIndex sync detects changes incrementally. It probes git state
and computes a diff against the last snapshot at [build_index](src/index.rs#L10-L20)
and again at [build_index](src/index.rs#L10-L20).

## Apply phase

The indexer applies each diff entry to the in-memory tree; the implementation
spans [apply_changes](src/index.rs#L25-L40) and [apply_changes_batch](src/index.rs#L45-L60).

## Cache layer

The cache wiki section describes the LRU cache used by index lookups,
backed by [CacheKey](src/index.rs#L70-L80).
