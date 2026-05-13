# Git Hook Setup

A two-phase git hook configuration that blocks commits with broken wiki links, then scaffolds mesh coverage for newly introduced fragment links after the commit lands.

---

## Design

Wiki validation is split across two hooks because the two error classes have different timing requirements:

| Error class | Severity | Hook | Action |
|---|---|---|---|
| Broken links, bad frontmatter, missing titles | **Blocking** | `pre-commit` | Reject the commit |
| `mesh_uncovered` (missing git mesh coverage) | **Deferred** | `post-commit` | Auto-scaffold |

**Why mesh coverage is deferred.** Mesh scaffolding generates `git mesh add` commands that require human review: anchors must be consolidated into meaningful meshes, and each mesh needs a real `why` sentence. The scaffold output is a starting point, not a finished product. Running it post-commit prints the commands so the committer can act on them without blocking the commit.

**Why link validation is blocking.** Broken links, missing titles, and stale fragment-link line ranges make the wiki incorrect. These are machine-detectable correctness errors with no judgment call involved — they should never enter the tree.

---

## Pre-Commit: Block on Validation

```bash
#!/bin/bash
set -e

WIKI_BIN=$(command -v wiki || true)
if [ -n "$WIKI_BIN" ]; then
    "$WIKI_BIN" check --no-mesh || {
        exit 1
    }
fi
```

`--no-mesh` skips the git mesh coverage check so mesh scaffolding is not required before the commit exists. All other checks (frontmatter, wikilinks, fragment-link SHAs, line-range bounds) run and fail the commit on error.

**Why check all wiki pages, not just staged files.** A staged edit can break a wikilink on an unstaged page, or introduce a title collision with a page that was not touched. Checking the entire corpus catches these cross-page failures.

If `wiki` is not installed (`command -v` fails), the hook silently passes — wiki validation is a local development guard, not a CI gate.

---

## Post-Commit: Scaffold Mesh Coverage

```bash
WIKI_BIN=$(command -v wiki || true)
if [ -n "$WIKI_BIN" ]; then
  WIKI_JSON=$("$WIKI_BIN" check --format json --no-exit-code 2>&1) || true

  if command -v jq >/dev/null 2>&1 && [ -n "$WIKI_JSON" ]; then
    MESH_FILES=$(echo "$WIKI_JSON" \
      | jq -r '[.errors[] | select(.kind == "mesh_uncovered") | .file] | unique | .[]' \
      2>/dev/null || true)
    if [ -n "$MESH_FILES" ]; then
      MESH_ARGS=$(echo "$MESH_FILES" | tr '\n' ' ')
      # shellcheck disable=SC2086
      "$WIKI_BIN" scaffold $MESH_ARGS
    fi
  fi
fi
```

`--format json` produces structured output that `jq` can filter. `--no-exit-code` ensures the post-commit never fails — linking errors that would have been caught pre-commit only appear here if the pre-commit hook is missing, and mesh coverage gaps are expected for newly committed pages.

**The jq filter** selects errors where `kind == "mesh_uncovered"`, extracts the `file` field, and deduplicates. All collected files are passed to `wiki scaffold` in a single invocation.

**The scaffold output** is a shell script of `git mesh add` and `git mesh why` commands. It is printed to stdout for the committer to review, consolidate, and commit separately — it is not executed automatically.
