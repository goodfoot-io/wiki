# wiki

A fast, Rust-powered wiki toolkit for local-first Markdown knowledge bases. This monorepo ships:

- **`@goodfoot/wiki`** — a standalone CLI for indexing, searching, linking, and rendering Markdown wikis
- **Wiki Viewer** — a VS Code extension that renders wiki pages with live markdown-link navigation
- **Agent plugins** — ready-to-install plugins for Claude Code, Codex, and OpenCode that teach coding agents to read and write the wiki

Documentation is stored as plain Markdown with relative-path links and optional frontmatter — nothing proprietary, no database you can't read.

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

# Find stale or broken references
wiki check
wiki stale

# Extract frontmatter, summaries, pinned articles, or headings
wiki extract ...
wiki summary "Authorization"
wiki pin list
wiki list
```

### Agent integration

The wiki validates itself through coding-agent plugins built from a single hook
source: [packages/agent-hooks/](./packages/agent-hooks) owns one TypeScript
PostToolUse implementation, compiled into all three plugin trees, whose hook
shells out to `wiki check --fix` whenever an agent writes or edits a Markdown
file. The wiki skill every plugin ships is authored once at
[skills/wiki/](./skills/wiki/) and symlinked into each tree.

- **Claude Code** — install the `wiki` plugin from the local marketplace defined
  in [.claude-plugin/marketplace.json](./.claude-plugin/marketplace.json)
- **Codex** — install from the local marketplace defined in
  [.agents/plugins/marketplace.json](./.agents/plugins/marketplace.json)
- **OpenCode** — register [`plugins-opencode/wiki/`](./plugins-opencode/wiki/)
  via the `plugin` array in `opencode.json`, or install the npm-ready
  **@goodfoot/opencode-wiki** package

To keep the index synchronized outside an agent, copy the sample git hooks from
[examples/githooks/](./examples/githooks/) into `.git/hooks/` — see
[examples/githooks/README.md](./examples/githooks/README.md).

### Features

- **Markdown link resolution** — relative-path links between wiki pages resolved against the linking file's directory
- **FTS5 full-text search** — powered by an embedded SQLite (turso) index with BM25 ranking and snippet extraction
- **Fragment links** — heading slugs are stable and addressable; `#heading` fragments survive rename
- **Git-aware indexing** — the sample git hooks keep the index in lockstep with commits and merges (WAL mode for concurrent access)
- **Frontmatter-driven** — title, aliases, tags, and summaries read from YAML frontmatter
- **Syntax highlighting** — pure-Rust syntect + fancy-regex (no native C dependency)
- **Fail-closed validation** — `wiki check` surfaces broken links and missing targets as errors

## Wiki Viewer (VS Code extension)

Install **Wiki Viewer** from the [VS Code Marketplace](https://marketplace.visualstudio.com/) or [Open VSIX](https://open-vsx.org/). It registers as the default editor for `.md` files whose YAML frontmatter has both a non-empty `title` and a non-empty `summary`; other markdown files fall through to VS Code's default editor.

### Features

- Rendered Markdown webview with wikilink navigation and fragment support
- Ranked wiki search (`Shift+Cmd+L` / `wiki.search`)
- Seamless switch between rendered and source views (`wiki.openInEditor`)
- Syntax-highlighted code blocks, morphdom-powered incremental updates

## Monorepo layout

```
.
├── packages/
│   ├── cli/            # @goodfoot/wiki — Rust CLI (Cargo workspace root)
│   ├── extension/      # Wiki Viewer VS Code extension (TypeScript)
│   └── agent-hooks/    # @goodfoot/agent-hooks — single hook source for all three plugins
├── npm/
│   └── wiki-*/         # Platform-specific binary distribution packages
├── plugins-claude/
│   └── wiki/           # Claude Code plugin: generated hooks + symlinked skills
├── plugins-codex/
│   └── wiki/           # Codex plugin: generated hooks + symlinked skills
├── plugins-opencode/
│   └── wiki/           # @goodfoot/opencode-wiki — OpenCode plugin and npm package
├── skills/
│   └── wiki/           # Single-source wiki skill, shared by every plugin tree
├── examples/
│   └── githooks/       # Sample post-commit / post-merge hooks for Claude, Gemini
├── docs/
│   └── cross-compilation.md
└── scripts/
    ├── sync-versions.sh
    ├── validate.sh
    └── release.sh
```

The Rust CLI lives in `packages/cli/`; `packages/cli/package.json` is the single source of truth for the version (propagated by `scripts/sync-versions.sh`). The VS Code extension is a standard TypeScript package built with esbuild and packaged via `vsce`.

## Agent plugins

One hook source builds three platform plugins. `packages/agent-hooks/` owns the
PostToolUse implementation, and `yarn workspace @goodfoot/agent-hooks build`
emits the generated bundles committed under `plugins-claude/wiki/hooks/`,
`plugins-codex/wiki/hooks/`, and `plugins-opencode/wiki/dist/` — validation
fails closed if a rebuild ever leaves one of them stale. Each tree provides:

- **Hooks** — a `PostToolUse` hook that runs `wiki check --fix` after an agent writes or edits a Markdown file, keeping links and frontmatter valid
- **Skills** — the wiki skill authored once at [skills/wiki/](./skills/wiki/) and shared through relative symlinks, teaching agents how to query and author wiki articles using the CLI

Install per platform:

- **Claude Code** — [.claude-plugin/marketplace.json](./.claude-plugin/marketplace.json) exposes the plugin at [plugins-claude/wiki/](./plugins-claude/wiki/) through the Claude Code marketplace integration
- **Codex** — [.agents/plugins/marketplace.json](./.agents/plugins/marketplace.json) exposes the plugin at [plugins-codex/wiki/](./plugins-codex/wiki/) through the matching codex flow; review its hooks once via `/hooks` (fail-closed trust)
- **OpenCode** — [opencode.json](./opencode.json) registers [`plugins-opencode/wiki/`](./plugins-opencode/wiki/) in place; that directory doubles as the npm-publishable **@goodfoot/opencode-wiki** package

Or copy `examples/githooks/*.sh` into `.git/hooks/` for a minimal manual setup. See [examples/githooks/README.md](./examples/githooks/README.md) for the manual install instructions.

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
