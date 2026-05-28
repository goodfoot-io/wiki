---
title: Wiki Performance Optimization
summary: Strategies for maintaining fast wiki operations as the knowledge base grows.
---

The wiki CLI maintains performance through caching, incremental indexing, and parallel discovery, ensuring that operations stay responsive even as the number of pages grows.

## Strategy: Index Everything, Query Locally

To keep wiki interactions fast, the `wiki` tool maintains a local [SQLite index](/packages/cli/src/index/schema.rs) (`.git/wiki-index.sqlite`). This index caches page titles, aliases, summaries, and full-text content via an FTS5 virtual table. Most CLI commands query this index rather than parsing markdown files on every invocation.

## Optimizations

### 1. Parallel File Discovery

The [discovery process](/packages/cli/src/commands/mod.rs) uses a parallel directory walk (`ignore::WalkBuilder::build_parallel`) to enumerate markdown files across the repository. This significantly reduces the time spent on filesystem metadata operations, especially in large monorepos.

### 2. Git-Accelerated Inventory

When possible, the wiki [uses Git's index](/packages/cli/src/git.rs) to resolve default file lists. This avoids a full filesystem walk by leveraging Git's own tracking of repository content.

### 3. Incremental Indexing

The [WikiIndex refresh](/packages/cli/src/index/passes/mod.rs) detects changes incrementally across three passes (committed tree, git index, worktree). A stat-only [fast-triple gate](/packages/cli/src/index/freshness.rs) (HEAD OID, index checksum, worktree generation) lets warm searches skip opening a `gix::Repository` entirely.

### 4. Native FTS5 Body Indexing

Full-text search is [built into the SQLite schema](/packages/cli/src/index/schema.rs) as an external-content FTS5 virtual table over `blobs`. There is no deferred or separate search index — every blob inserted into `blobs` is tokenized into `fts` by the bootstrap triggers, so search is always live.

### 5. Weighted Search Ranking

To keep search performance high while improving relevance, [weighted search ranking](/packages/cli/src/index/search.rs) combines exact title and alias matches, path-fragment lookups, and a BM25 column cascade over `title`, `aliases_text`, `tags_text`, `keywords_text`, `summary`, and `body`. Each pass is optimized separately (B-tree lookups for titles and paths before falling back to BM25), ensuring that common navigational searches are nearly instantaneous.

See also: [Wiki CLI](../architecture/wiki-cli.md)
