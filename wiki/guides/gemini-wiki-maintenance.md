---
title: Gemini Wiki Maintenance
summary: How the automated post-commit wiki maintenance script works — what it does, when it runs, how it is isolated, the policy engine constraints, the two-path merge model, and how to diagnose failures.
tags:
  - wiki
  - automation
  - gemini
---

# Gemini Wiki Maintenance

The repository runs an automated wiki maintenance pass after every commit on the main workspace. [`examples/githooks/scripts/gemini-wiki-maintenance.sh`](/examples/githooks/scripts/gemini-wiki-maintenance.sh#e2b1474) invokes Gemini CLI to inspect stale fragment links, update prose, re-pin links, and commit the result — all without human intervention.

## When It Runs

The [example post-commit hook](/examples/githooks/gemini-post-commit.sh#e2b1474) fires the script in a background process (`&`) after every commit. A guard on the `GEMINI_WIKI_ACTIVE` environment variable prevents it from recursing into its own wiki commits.

A fail-closed lockfile under `$GIT_COMMON_DIR/wiki-maintenance.lock` ensures only one run is active at a time. Stale locks (from crashed processes) are detected by checking whether the PID is still alive.

The script exits immediately (before the expensive worktree setup) if `wiki check` reports no validation errors.

## What It Does

1. **Creates an isolated git worktree** on a temporary branch (`wiki-maintenance/<timestamp>`) pointing to the current `HEAD`. Gemini works inside this worktree — its changes are committed there before being merged back.

2. **Runs Gemini** inside the worktree with an explicit admin-level policy file and a structured multi-step prompt that enforces [**fragment link discipline**](/examples/githooks/scripts/gemini-wiki-maintenance.sh#L183-L193&e2b1474). Gemini's home directory is isolated (`$ISOLATED_HOME`) and its environment is strictly controlled, though `PATH` is forwarded so the `wiki` binary and other essential tools remain available. It uses separate credentials and cannot write outside the worktree.

3. **Verifies the result** with `wiki check` before accepting any changes. If the wiki is invalid after Gemini runs, the maintenance is aborted.

4. **Merges or patches** back into the main working tree:
   - **Clean working directory** — `git merge` (fast-forward) from the temp branch.
   - **Dirty working directory** — `git apply` of the patch, followed by an automatic `git commit` of the affected files using the generated message.

5. **Cleans up** the worktree, temp branch, lockfile, and temp files in an `EXIT` trap.

## Policy Engine

Gemini runs with a strict TOML admin-level policy (`$ISOLATED_HOME/wiki-policy.toml`) that uses a deny-first design: only explicitly allowed operations are permitted.

| What is allowed | Why |
|---|---|
| `write_file` / `replace` to `wiki/` paths | Wiki edits |
| `write_file` to `gemini-commit-message.txt` | Commit message output |
| Read-only tools (`readOnlyHint = true`) | MCP tools annotated as safe |
| `run_shell_command` matching `commandRegex` | wiki CLI, git read-only ops, cat, grep, find, ls, etc. |
| Everything else | **Denied** |

The `commandRegex` field generates an `argsPattern` of the form `"command":"<regex>"` — a substring match against the stable-JSON-serialized args. The `^` anchor must not be used here, because the pattern is not anchored to the start of the JSON string.

## Commit Message

Step 6 of the prompt instructs Gemini to write the commit message to `$WORKTREE_DIR/gemini-commit-message.txt` via `write_file`. The script uses that file if it exists and is non-empty; otherwise it falls back to `"wiki: maintenance pass — automated wiki maintenance"`.

## Logging

All output (stdout and stderr) is tee'd to `/workspace/.cards/logs/gemini-wiki-maintenance.log`. Each run is delimited by timestamp+PID banners. Failures include the exit code.

```
--- 2026-04-08 19:35:47 wiki maintenance run started (PID 88769, cwd=/workspace) ---
...
--- 2026-04-08 19:50:45 wiki maintenance run complete ---
```

## Diagnosing Failures

| Symptom | Likely cause |
|---|---|
| `Wiki maintenance is already running.` | Previous run holds the lock — check `cat .git/wiki-maintenance.lock/pid`, then `kill -0 <pid>` |
| `Gemini exited with error` | Policy denial from an unexpected tool call; check the log for `Tool execution denied` lines with the `denyMessage` |
| `Wiki check failed after agent run` | Gemini produced an invalid wiki state; the worktree is discarded |
| `Wiki update conflicted with local changes` | A wiki file was locally modified; the patch did not apply cleanly — retry after committing or stashing |
| Timeout (`exit 124`) | Gemini took longer than 900 s; increase `timeout 900` in the script or simplify the maintenance task |

## References

- [Gemini wiki maintenance script](/examples/githooks/scripts/gemini-wiki-maintenance.sh#e2b1474)
- [Gemini post-commit hook](/examples/githooks/gemini-post-commit.sh#e2b1474)