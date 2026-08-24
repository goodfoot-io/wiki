---
title: Wiki Performance Optimization
links-reviewed: 1
summary: Strategies for maintaining fast wiki operations as the knowledge base grows.
---

The wiki CLI maintains performance through caching, incremental indexing, and parallel discovery, ensuring that operations stay responsive even as the number of pages grows.

## Strategy: Index Everything, Query Locally

To keep wiki interactions fast, the `wiki tool` maintains a local [SQLite generations store](/packages/cli/src/index/generations.rs) (`$GIT_COMMON_DIR/wiki/store.sqlite`). This store caches page titles, aliases, summaries, and full-text content in per-generation FTS5 tables. Most CLI commands query this store rather than parsing markdown files on every invocation.

## Optimizations

### 1. Parallel File Discovery

The [discovery process](/packages/cli/src/commands/mod.rs) uses a parallel directory walk (`ignore::WalkBuilder::build_parallel`) to enumerate markdown files across the repository. This significantly reduces the time spent on filesystem metadata operations, especially in large monorepos.

### 2. Git-Accelerated Inventory

When possible, the wiki [uses Git's index](/packages/cli/src/git.rs) to resolve default file lists. This avoids a full filesystem walk by leveraging Git's own tracking of repository content.

### 3. Incremental Indexing

The [WikiIndex refresh](/packages/cli/src/index/passes/mod.rs) detects changes incrementally across three passes (committed tree, git index, worktree), diffing each against the inputs recorded by the base generation. A stat-only [digest gate](/packages/cli/src/index/freshness.rs#L23-L66) (canonical fingerprint over HEAD OID, index checksum, wikiignore hash, worktree signature) lets warm searches skip opening a `gix::Repository` entirely.

### 4. Per-Generation FTS5 Body Indexing

Full-text search is [materialized per generation](/packages/cli/src/index/generations.rs) as standalone FTS5 tables (`fts_{gen_id}`) populated inside the publish transaction. There is no deferred or separate search index — every generation publishes with its full corpus tokenized exactly as a cold build would produce it, so search is always live and warm rankings equal cold rebuilds.

### 5. Membership Refcounts, Reconciled Set-Based

The `blobs` table is content-addressed: one row per unique blob, with `gen_paths` rows from up to three sources (committed tree, git index, worktree) pointing at it. Each blob carries a membership refcount — the number of retained generations referencing it — and every transaction that mutates `gen_paths` (publish, conflict-discard, GC eviction) reconciles affected refcounts set-based in the same transaction, so a blob can never drift from the rows that cite it and is reclaimed only when its last citing generation is evicted.

A per-refresh blob cache in the [delta apply loop](/packages/cli/src/index/passes/mod.rs) complements this: blob existence is tracked in memory rather than queried per delta, and a non-wiki blob is read and parsed at most once per refresh, no matter how many sources mention it.

### 6. Weighted Search Ranking

To keep search performance high while improving relevance, [weighted search ranking](/packages/cli/src/index/search.rs) combines exact title and alias matches, path-fragment lookups, and a BM25 column cascade over `title`, `aliases_text`, `tags_text`, `keywords_text`, `summary`, and `body`. Each pass is optimized separately (B-tree lookups for titles and paths before falling back to BM25), ensuring that common navigational searches are nearly instantaneous.

See also: [Wiki CLI](../architecture/wiki-cli.md)
