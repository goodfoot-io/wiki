# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Added
- Initial release: migrated from internal monorepo to standalone repository
- `wiki check` classifies every line-range link against the page's
  git-derived anchor epoch — the cited file's content at the commit where
  `links-reviewed:` last changed — as healthy, uncertified, broken, drifted,
  moved, or unverifiable. `--fix` relocates links whose certified content
  moved, routes broken targets through the rename machinery, and initializes
  `links-reviewed:` on field-less pages. The anchor-commit lookup walks full
  history and fails closed on shallow clones; CI runs `wiki check` fail-closed
  and checks out with `fetch-depth: 0`.

### Changed
- The pre-commit wiki hook runs a single best-effort
  `wiki check --fix --print-applied` pass and stages exactly the files the run
  rewrote, replacing the blob-hash snapshot pass and the mesh scaffolding.

### Removed
- The `wiki mesh` subcommand family (`show`, `add`, `remove`, `merge`), the
  `.wiki/<slug>.mesh` anchor-file corpus, and the wiki-mesh merge driver.
  Line-range drift detection is now git-derived instead: each page's
  `links-reviewed:` frontmatter value marks its anchor epoch.
- `wiki html` and `wiki serve` subcommands and their renderer. HTML rendering
  now lives entirely in the VS Code extension webview. The page titles `html`
  and `serve` are no longer reserved and can be used as wiki page titles.
  Dropped the `axum`, `pulldown-cmark`, `syntect`, `tokio-stream`, and `notify`
  dependencies, and trimmed `tokio` to the `rt` feature.
