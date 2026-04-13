# Wiki Git Hooks

Example git hooks that use AI coding agents to automatically maintain wiki documentation. Each hook runs in the background after commits or merges, detecting documentation gaps and updating stale wiki pages.

## Available Providers

| Provider | CLI | Best for |
|----------|-----|----------|
| **Gemini** | `gemini` | Built-in sandboxing via admin policy files; good for automated/CI use |
| **Claude** | `claude` | Strong at nuanced prose and cross-reference analysis |
| **Codex** | `codex` | Full-auto approval mode; good for unattended batch runs |

## Prerequisites

1. **wiki CLI** on your PATH (install from this repo: `packages/cli`)
2. Your chosen AI CLI installed and authenticated:
   - Gemini: `npm install -g @google/gemini-cli` + API key configured
   - Claude: `npm install -g @anthropic-ai/claude-code` + API key configured
   - Codex: `npm install -g @openai/codex` + API key configured
3. Git repository with a `wiki/` directory containing markdown documentation

## Installation

### Option A: Copy hooks directly

```bash
mkdir -p .githooks

# Pick your provider (example: claude)
cp examples/githooks/claude-post-commit.sh .githooks/post-commit
cp examples/githooks/claude-post-merge.sh .githooks/post-merge

# Copy the scripts directory
cp -r examples/githooks/scripts .githooks/scripts

# Make everything executable
chmod +x .githooks/post-commit .githooks/post-merge .githooks/scripts/*.sh

# Configure git to use the hooks directory
git config core.hooksPath .githooks
```

### Option B: Symlink

```bash
mkdir -p .githooks
ln -s ../examples/githooks/claude-post-commit.sh .githooks/post-commit
ln -s ../examples/githooks/claude-post-merge.sh .githooks/post-merge
git config core.hooksPath .githooks
```

## Configuration

Each script supports these environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `WIKI_DIR` | `wiki` | Wiki directory relative to repo root |
| `LOG_DIR` | `.wiki/logs` | Log directory relative to repo root |

Set them in your shell profile or export before running git commands:

```bash
export WIKI_DIR="docs/wiki"
export LOG_DIR=".wiki/logs"
```

## How It Works

### Gap Detection (post-commit)

1. Tracks which commits have been scanned via a state file
2. Finds source files (`.rs`, `.ts`) changed since the last scan
3. Checks each file for wiki fragment link coverage using `wiki refs`
4. Applies an inclusion gate (must synthesize across boundaries, not just document a single file)
5. Creates an isolated git worktree for the AI agent to work in
6. Agent creates or expands wiki pages with proper fragment links
7. Verifies with `wiki check` before merging changes back

### Maintenance (post-commit, post-merge)

1. Runs `wiki stale` to find outdated fragment links
2. For each stale page, the AI agent reads the code diff
3. Classifies changes (behavior, boundary, cosmetic, deletion)
4. Updates prose and re-pins fragment links
5. Verifies with `wiki check` and `wiki stale` before merging

### Safety Features

- **Recursion prevention**: Environment variables prevent hooks from triggering during their own commits
- **Single-flight lock**: Only one instance of each script runs at a time (lockfile with PID check)
- **Isolated worktree**: All AI changes happen in a temporary worktree, not in your working directory
- **Post-run verification**: `wiki check` must pass before any changes are merged back
- **Two-path merge**: Clean working directories get a fast-forward merge; dirty directories get a patch apply with conflict detection
- **Timeout**: Each AI invocation has a 15-minute timeout

## Logs

Check logs in your configured `LOG_DIR` (default `.wiki/logs/`):

```bash
tail -f .wiki/logs/claude-wiki-gap-detection.log
tail -f .wiki/logs/claude-wiki-maintenance.log
```
