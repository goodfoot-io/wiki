---
title: Wikilink Resolution
summary: How the wiki resolves [[README]] references to canonical pages and handles alias matching.
---

Wikilinks are the mechanism by which wiki pages reference each other. The resolution system handles both canonical titles and aliases, ensuring that pages can link to each other using human-readable names rather than file paths.

## Indexing and Storage

When the [WikiIndex](packages/cli/src/index.rs#L173-L190) is constructed, it scans wiki pages to extract titles, optional aliases, and [[Wiki CLI|fragment links]]. This data is indexed in a SQLite database to support O(1) wikilink resolution and unified incoming-link discovery via the `wiki links` command.

## Collision Detection

The [check command](packages/cli/src/commands/check.rs#L28-L29) validates that no two pages share the same title or alias. Title collisions would create ambiguity in wikilink resolution and break the assumption that each concept has exactly one canonical home.

## Wikilink Extraction and Resolution

The [extract command](packages/cli/src/commands/extract.rs#23a197090) reads text from stdin, parses all `[[wikilink]]` references using a parser that operates on scrubbed content (code blocks and inline code are blanked out to avoid false matches), and outputs the canonical title and summary for each resolved page. Unresolved wikilinks are reported to stderr with exit code 1, signaling an error that should be fixed.

## Fragment Link Anchoring

While wikilinks reference other pages, [[Wiki CLI|fragment links]] anchor prose to specific code locations. The two link types serve different purposes: wikilinks navigate the conceptual domain, while fragment links detect staleness and provide evidence for claims.

See also: [[Wiki Organization]]