# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Added
- Initial release: migrated from internal monorepo to standalone repository

### Removed
- `wiki html` and `wiki serve` subcommands and their renderer. HTML rendering
  now lives entirely in the VS Code extension webview. The page titles `html`
  and `serve` are no longer reserved and can be used as wiki page titles.
  Dropped the `axum`, `pulldown-cmark`, `syntect`, `tokio-stream`, and `notify`
  dependencies, and trimmed `tokio` to the `rt` feature.
