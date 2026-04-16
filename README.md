# wiki

A fast, Rust-powered wiki toolkit for local-first Markdown knowledge bases. This monorepo ships:

- **`@goodfoot/wiki`** — a standalone CLI for indexing, searching, linking, and rendering Markdown wikis
- **Wiki Viewer** — a VS Code extension that renders wiki pages with live wikilink navigation
- **Agent plugins** — ready-to-install Claude Code and Codex plugins that teach coding agents to read and write the wiki

Documentation is stored as plain Markdown with `[[wikilink]]` references and optional frontmatter — nothing proprietary, no database you can't read.

## @goodfoot/wiki (Rust CLI)

### Install

```bash
npm install -g @goodfoot/wiki
```

The package ships prebuilt binaries for Linux (x64, arm64), macOS (x64, arm64), and Windows (x64). A `postinstall` script links the correct platform binary into `bin/wiki`. See [docs/cross-compilation.md](./docs/cross-compilation.md) for the target matrix and native dependency notes.

### Usage

```bash
# Search the wiki with FTS5 full-text search (ranked, snippet-aware)
wiki search "authorization"
wiki "Example Article"            # default: ranked title + summary lookup

# Resolve and print an article by title, alias, or path
wiki print "Authorization"
wiki print "Authorization#token-refresh"   # fragment link to a heading

# Enumerate wikilinks, find stale or broken references
wiki links path/to/article.md
wiki check
wiki stale

# Render an article to HTML (standalone or for the extension webview)
wiki html "Authorization"

# Extract frontmatter, summaries, pinned articles, or headings
wiki extract ...
wiki summary "Authorization"
wiki pin list
wiki list

# Run as a long-lived server for editor integrations
wiki serve

# Run inside a git hook (indexes new/changed articles)
wiki hook post-commit

# Install the Codex integration (skill, PostToolUse hook, feature flag)
wiki install --codex
```

### Installing the Codex integration

`wiki install --codex` downloads the latest plugin assets from
[goodfoot-io/wiki](https://github.com/goodfoot-io/wiki) and installs them into
your Codex home (resolved from `--codex-home`, `$CODEX_HOME`, or `~/.codex`). It
installs:

- The `wiki` skill under `$CODEX_HOME/skills/wiki/`
- A managed `PostToolUse` hook group in `$CODEX_HOME/hooks.json` that runs
  `wiki hook --codex`
- `[features].codex_hooks = true` in `$CODEX_HOME/config.toml`

Re-running the command updates the install in place: the managed skill
directory is replaced atomically, the managed hook group is upserted without
touching unrelated hook config, and `codex_hooks` is ensured. Backups of any
changed files are written under `$CODEX_HOME/.wiki-install/backups/`.

```bash
wiki install --codex                        # install or update from main
wiki install --codex --ref v1.0.2           # pin to a tag, branch, or SHA
wiki install --codex --codex-home ./codex   # override the Codex home
wiki install --codex --dry-run              # print planned changes, write nothing
wiki install --codex --force                # overwrite unmanaged skill/hook conflicts
wiki install --claude                       # print friendly Claude Code setup instructions
```

The command is fail-closed: if the download, archive validation, or existing
`hooks.json`/`config.toml` parse fails, no files are written.

Using Claude Code instead? Run `wiki install --claude` for a friendly,
copy-pasteable guide to adding the wiki plugin marketplace. That mode is
informational only — it never runs commands, fetches anything, or touches the
filesystem; you stay in control.

### Features

- **Wikilink resolution** — `[[Title]]`, `[[Title#heading]]`, `[[Title|alias]]` resolved against titles, aliases, and file paths
- **FTS5 full-text search** — powered by an embedded SQLite (turso) index with BM25 ranking and snippet extraction
- **Fragment links** — heading slugs are stable and addressable; `#heading` fragments survive rename
- **Git-aware hooks** — `wiki hook` phases keep the index in lockstep with commits, merges, and rebases (WAL mode for concurrent access)
- **Frontmatter-driven** — title, aliases, tags, and summaries read from YAML frontmatter
- **Syntax highlighting** — pure-Rust syntect + fancy-regex (no native C dependency)
- **Fail-closed validation** — `wiki check` surfaces broken links and missing targets as errors

## Wiki Viewer (VS Code extension)

Install **Wiki Viewer** from the [VS Code Marketplace](https://marketplace.visualstudio.com/) or [Open VSIX](https://open-vsx.org/). It registers as the default editor for `**/wiki/**/*.md` and `**/*.wiki.md`.

### Features

- Rendered Markdown webview with wikilink navigation, backlinks, and fragment support
- Ranked wiki search (`Shift+Cmd+L` / `wiki.search`)
- Seamless switch between rendered and source views (`wiki.openInEditor`)
- Configurable via `wiki.openFilesInViewer`
- Syntax-highlighted code blocks, morphdom-powered incremental updates

## Monorepo layout

```
.
├── packages/
│   ├── cli/            # @goodfoot/wiki — Rust CLI (Cargo workspace root)
│   └── extension/      # Wiki Viewer VS Code extension (TypeScript)
├── npm/
│   └── wiki-*/         # Platform-specific binary distribution packages
├── plugins/
│   └── wiki/           # Agent plugin: hooks + skills (see below)
├── examples/
│   └── githooks/       # Sample post-commit / post-merge hooks for Claude, Codex, Gemini
├── docs/
│   └── cross-compilation.md
└── scripts/
    ├── sync-versions.sh
    ├── validate.sh
    └── release.sh
```

The Rust CLI lives in `packages/cli/`; `packages/cli/package.json` is the single source of truth for the version (propagated by `scripts/sync-versions.sh`). The VS Code extension is a standard TypeScript package built with esbuild and packaged via `vsce`.

## Agent plugin marketplaces

The `plugins/wiki/` directory is a shared plugin distributed through both the **Claude Code** and **Codex** plugin marketplaces. It provides:

- **Hooks** — `post-commit` / `post-merge` handlers that keep the wiki index synchronized after agent-driven commits
- **Skills** — a `wiki` skill that teaches agents how to query and author wiki articles using the CLI, including `[[wikilink]]` conventions and frontmatter rules

Install via the marketplace integration in Claude Code or Codex, or copy `examples/githooks/*.sh` into `.git/hooks/` for a minimal manual setup. See [examples/githooks/README.md](./examples/githooks/README.md) for the manual install instructions.

## Quick start (contributors)

```bash
# Clone and install JavaScript workspace deps
git clone https://github.com/goodfoot-io/wiki.git
cd wiki
yarn install

# Build everything (Rust CLI + VS Code extension)
yarn build

# Run full validation: typecheck, lint, test, build (all packages)
yarn validate
```

Per-package validation runs from each package directory with its own `yarn lint`, `yarn typecheck`, and `yarn test` scripts. The Rust CLI uses dedicated `CARGO_TARGET_DIR` paths (`target/lint`, `target/test`, `target/typecheck`, `target/build`) so concurrent `cargo` commands don't contend on a shared lock. See [CLAUDE.md](./CLAUDE.md) and [AGENTS.md](./AGENTS.md) for contributor conventions.

Releases are tag-driven:

- `wiki-v*` triggers [`.github/workflows/release-cli.yml`](./.github/workflows/release-cli.yml), which publishes the CLI first and then invokes [`.github/workflows/release-extension.yml`](./.github/workflows/release-extension.yml) to package and publish the extension from the same tag

## License

MIT — Copyright (c) 2026 Goodfoot Media LLC. See [LICENSE](./LICENSE).
