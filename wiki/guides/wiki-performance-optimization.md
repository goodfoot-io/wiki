---
title: Wiki Performance Optimization
summary: Strategies for maintaining fast wiki operations as the knowledge base grows.
---

The wiki CLI maintains performance through caching, incremental indexing, and parallel discovery, ensuring that operations stay responsive even as the number of pages grows.

## Strategy: Index Everything, Query Locally

To keep wiki interactions fast, the `wiki` tool maintains a local SQLite index (`wiki/.index.db`). This index caches page titles, aliases, summaries, and full-text content. Most CLI commands query this index rather than parsing markdown files on every invocation.

## Optimizations

### 1. Parallel File Discovery

The [discovery process](packages/cli/src/commands/mod.rs#L261-L280) uses a parallel directory walk (`ignore::WalkBuilder::build_parallel`) to enumerate markdown files across the repository. This significantly reduces the time spent on filesystem metadata operations, especially in large monorepos.

### 2. Git-Accelerated Inventory

When possible, the wiki [uses Git's index](packages/cli/src/commands/mod.rs#L171-L190) to resolve default file lists. This avoids a full filesystem walk by leveraging Git's own tracking of repository content.

### 3. Incremental Indexing

The [WikiIndex sync](packages/cli/src/index.rs#L448-L460) detects changes incrementally. By [probing Git state](packages/cli/src/index.rs#L804-L850) (HEAD SHA, wiki dir, and working tree status), the indexer identifies which files have been added, modified, or deleted since the last sync. This avoids re-parsing every page when only a few have changed.

### 4. Deferred Search Index Rebuilds

Full-text search (FTS) indexing is [decoupled from the core document index](packages/cli/src/index.rs#L1022-L1040). Core indexing is optimized for speed, while the search index is rebuilt [in the background](packages/cli/src/index.rs#L1060-L1080) or lazily when a search query is executed. This ensures that most CLI commands remain responsive even after large wiki updates.

### 5. Weighted Search Ranking

To keep search performance high while improving relevance, [weighted search ranking](packages/cli/src/index.rs#L1315-L1350) combines exact title matches, path matches, and FTS results. Each pass is optimized separately (using B-tree lookups for titles and paths before falling back to BM25), ensuring that common navigational searches are nearly instantaneous.

### 6. Git Result Caching

The [stale command](packages/cli/src/commands/stale.rs#L66-L75) caches Git operation results (commits, stats, and patches). In pages with many fragment links to the same file at the same SHA, this eliminates redundant Git invocations.

### 7. Debounced Live Reloading

In [serve mode](packages/cli/src/commands/serve.rs#L93-L130), the file watcher uses a background worker thread with debouncing. This batches multiple file change events (e.g., from a branch switch or batch edit) into a single incremental index refresh, preventing rapid reload cycles.

See also: [[Wiki CLI]]