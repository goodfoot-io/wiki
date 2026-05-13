---
title: Wikilink Resolution
summary: How the wiki resolves page title references to canonical pages and handles alias matching.
---

Wikilinks are the mechanism by which wiki pages reference each other. The resolution system handles both canonical titles and aliases, ensuring that pages can link to each other using human-readable names rather than file paths.

## Indexing and Storage

When the [WikiIndex](/packages/cli/src/index.rs#L567-L577) is constructed, it scans wiki pages to extract titles, optional aliases, and [fragment links](./wiki-cli.md). This data is indexed in a SQLite database to support O(1) [wikilink resolution](/packages/cli/src/index.rs#L304-L307) and unified incoming-link discovery via the [`wiki links` command](/packages/cli/src/commands/links.rs#L115-L115).

## Collision Detection

The [check command](/packages/cli/src/commands/check.rs#L28-L29) validates that no two pages share the same title or alias. Title collisions would create ambiguity in wikilink resolution and break the assumption that each concept has exactly one canonical home.

## Wikilink Extraction and Resolution

The [extract command](/packages/cli/src/commands/extract.rs#L17-L17) reads text from stdin, [parses all wiki references](/packages/cli/src/parser.rs#L304-L304) using a parser that operates on [scrubbed content](/packages/cli/src/parser.rs#L79-L79) (code blocks and inline code are blanked out to avoid false matches), and outputs the canonical title and summary for each resolved page. Unresolved references are reported to stderr with exit code 1, signaling an error that should be fixed.

## Fragment Link Anchoring

While wikilinks reference other pages, [fragment links](./wiki-cli.md) anchor prose to specific code locations. The two link types serve different purposes: wikilinks navigate the conceptual domain, while fragment links detect staleness and provide evidence for claims.

See also: [Wiki Organization](../meta/wiki-organization.md)