---
title: Wiki Performance Optimization
summary: Strategies for maintaining fast wiki operations as the knowledge base grows.
---

The wiki CLI maintains performance through caching, incremental indexing, and parallel discovery, ensuring that operations stay responsive even as the number of pages grows.

## Strategy: Index Everything, Query Locally

To keep wiki interactions fast, the `wiki` tool maintains a local SQLite index (`wiki/.index.db`). This index caches page titles, aliases, summaries, and full-text content. Most CLI commands query this index rather than parsing markdown files on every invocation.

## Optimizations

### 1. Parallel File Discovery

The [discovery process](packages/cli/src/commands/mod.rs#L261-L280&628d6f9) uses a parallel directory walk (`ignore::WalkBuilder::build_parallel`) to enumerate markdown files across the repository. This significantly reduces the time spent on filesystem metadata operations, especially in large monorepos.

### 2. Git-Accelerated Inventory

When possible, the wiki [uses Git's index](packages/cli/src/commands/mod.rs#L184-L211&9b91dfb) to resolve default file lists. This avoids a full filesystem walk by leveraging Git's own tracking of repository content.

### 3. Incremental Indexing

The [WikiIndex sync](packages/cli/src/index.rs#L945-L953&9b91dfb) detects changes incrementally. By [probing Git state](packages/cli/src/index.rs#L960-L995&9b91dfb) (HEAD SHA, wiki dir, and working tree status), the indexer identifies which files have been added, modified, or deleted since the last sync. This avoids re-parsing every page when only a few have changed.

### 4. Deferred Search Index Rebuilds

Full-text search (FTS) indexing is [decoupled from the core document index](packages/cli/src/index.rs#L912-L926&9b91dfb). Core indexing is optimized for speed, while the search index is rebuilt [lazily when a search query is executed](packages/cli/src/index.rs#L1525-L1565&9b91dfb). This ensures that most CLI commands remain responsive even after large wiki updates.

### 5. Weighted Search Ranking

To keep search performance high while improving relevance, [weighted search ranking](packages/cli/src/index.rs#L1362-L1410&9b91dfb) combines exact title matches, path matches, and FTS results. Each pass is optimized separately (using B-tree lookups for titles and paths before falling back to BM25), ensuring that common navigational searches are nearly instantaneous.

### 6. Debounced Live Reloading

In [serve mode](packages/cli/src/commands/serve.rs#L93-L130&6a486f7), the file watcher uses a background worker thread with debouncing. This batches multiple file change events (e.g., from a branch switch or batch edit) into a single incremental index refresh, preventing rapid reload cycles.

See also: [[Wiki CLI]]