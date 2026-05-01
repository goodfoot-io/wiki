---
title: Manual Testing Procedure
summary: End-to-end manual smoke test of every wiki CLI feature, starting from an empty temp directory.
tags: [testing, cli, guide]
---

# Manual Testing Procedure

A linear walkthrough that exercises every `wiki` CLI subcommand and flag from a clean state. Each step prints the command, the expected outcome, and (where relevant) the expected exit code. Copy-paste into a shell, top to bottom.

Assumes `wiki` is on `PATH` (`which wiki` should resolve). Does not require an existing repo — the procedure builds one.

## 0. Setup — empty temp repo

```bash
WORK=$(mktemp -d) && cd "$WORK"
git init -q
echo "$WORK"
```

Expect: `git init` reports a new repo. `pwd` is the temp dir. No `wiki.toml` exists yet.

## 1. `wiki` with no config

```bash
wiki "anything" ; echo "exit:$?"
```

Expect: error `no wiki.toml found under <WORK>; run 'wiki init' …`, exit `2`.

```bash
wiki namespaces ; echo "exit:$?"
```

Expect: same error, exit `2`.

## 2. `wiki init`

### 2a. default namespace

```bash
mkdir docs && cd docs
wiki init ; cat wiki.toml ; echo "exit:$?"
cd ..
```

Expect: `created <…>/docs/wiki.toml`. `wiki.toml` is empty (default namespace). Exit `0`.

### 2b. duplicate refused

```bash
( cd docs && wiki init ) ; echo "exit:$?"
```

Expect: `wiki.toml already exists … remove it first if you want to reinitialise`. Exit `2`.

### 2c. named namespace

```bash
mkdir notes && ( cd notes && wiki init scratch && cat wiki.toml ) ; echo "exit:$?"
```

Expect: `wiki.toml` contains `namespace = "scratch"`. Exit `0`.

### 2d. invalid names rejected

```bash
mkdir bad1 && ( cd bad1 && wiki init "default" ) ; echo "exit:$?"
mkdir bad2 && ( cd bad2 && wiki init "bad name" ) ; echo "exit:$?"
mkdir bad3 && ( cd bad3 && wiki init "" )         ; echo "exit:$?"
```

Expect: each errors with the relevant rule (`reserved`, `only ASCII letters, digits, _, -`, `must not be empty`). Each exits `2`.

## 3. `wiki namespaces`

```bash
wiki namespaces ; echo "exit:$?"
```

Expect: two tab-separated rows, exit `0`:

```
default	<WORK>/docs
scratch	<WORK>/notes
```

`default` always sorts first; remaining namespaces are alphabetical.

### 3a. JSON

```bash
wiki namespaces --format json
```

Expect: a JSON array with two objects; the default has `"namespace": null`, the named has `"namespace": "scratch"`. Both have `"status": "ok"`.

### 3b. Duplicate-namespace fail-closed

```bash
mkdir dupA dupB
echo 'namespace = "dupe"' > dupA/wiki.toml
echo 'namespace = "dupe"' > dupB/wiki.toml
wiki namespaces ; echo "exit:$?"
rm -rf dupA dupB
```

Expect: `error: namespace 'dupe' declared by both …` on stderr; the two duplicate rows still print on stdout; exit `1`.

### 3c. Parse-error row visible

```bash
mkdir broken && echo 'invalid !!!' > broken/wiki.toml
wiki namespaces ; echo "exit:$?"
rm -rf broken
```

Expect: a third tab column shows the parse error on the broken row; exit `1` (other namespaces still listed).

## 4. Authoring content

### 4a. central wiki page (default namespace)

```bash
cat > docs/authentication.md <<'EOF'
---
title: Authentication
aliases: [auth, login]
tags: [security, infra]
summary: How the system authenticates users.
---

# Authentication

We use OAuth2. See [[Sessions]] and [[scratch:Operator Notes]].
EOF

cat > docs/sessions.md <<'EOF'
---
title: Sessions
tags: [security]
summary: Session lifecycle.
---

# Sessions

Sessions are cookies, refreshed via [[Authentication]].
EOF
```

### 4b. fragment file (lives next to code)

```bash
mkdir -p src/auth
cat > src/auth/oauth.rs <<'EOF'
// stub
fn login() {}
EOF

cat > src/auth/oauth.wiki.md <<'EOF'
---
title: OAuth Notes
summary: Implementation notes for the OAuth client.
---

Implements the flow described in [[Authentication]].
Anchors: src/auth/oauth.rs#L1-L2
EOF
```

### 4c. peer-namespace page

```bash
cat > notes/operator-notes.md <<'EOF'
---
title: Operator Notes
summary: Day-2 operations runbook.
---

Cross-references [[default:Authentication]].
EOF
```

### 4d. commit so git mesh can run later

```bash
git add . && git -c commit.gpgsign=false commit -q -m "seed wiki content"
```

## 5. `wiki list`

```bash
wiki list ; echo "exit:$?"
```

Expect: every default-namespace page (`Authentication`, `Sessions`, `OAuth Notes`) with title, aliases, tags, file path. Exit `0`.

```bash
wiki -n scratch list
```

Expect: only `Operator Notes`. Exit `0`.

```bash
wiki -n '*' list
```

Expect: pages from both namespaces, each row prefixed with its namespace.

## 6. `wiki "<query>"` (search)

### 6a. default namespace

```bash
wiki "OAuth"
wiki "session"
```

Expect: ranked matches with snippets. The default exit is `0`.

### 6b. limit and offset

```bash
wiki "Authentication" -l 1
wiki "Authentication" -l 1 -o 1
```

Expect: page 1 then page 2 of results.

### 6c. peer namespace

```bash
wiki -n scratch "operator"
wiki "@scratch operator"   # @-sugar
```

Expect: both invocations match `Operator Notes` from the `scratch` wiki.

### 6d. multi-namespace

```bash
wiki -n '*' "Authentication"
```

Expect: rows labelled by namespace; both wikis searched.

### 6e. unknown namespace fail-closed

```bash
wiki -n unknown "x" ; echo "exit:$?"
```

Expect: `unknown namespace 'unknown'. Known: [default, scratch]`. Exit `2`.

### 6f. JSON

```bash
wiki "OAuth" --format json
```

Expect: JSON array of result objects.

## 7. `wiki summary`

```bash
echo "docs/authentication.md" | wiki summary
wiki summary "Authentication"   # by title
wiki summary "auth"             # by alias
```

Expect: prints the page's `summary` frontmatter field.

```bash
wiki summary "DoesNotExist" ; echo "exit:$?"
```

Expect: error, exit non-zero.

## 8. `wiki refs`

```bash
wiki refs "Authentication"
```

Expect: metadata for every wikilink referenced from `docs/authentication.md` — at minimum `Sessions` and `scratch:Operator Notes`.

## 9. `wiki links`

```bash
wiki links "Authentication"
```

Expect: pages that link **to** `Authentication`, including `Sessions` (`docs/sessions.md`) and `OAuth Notes` (`src/auth/oauth.wiki.md`) and `scratch:Operator Notes`.

## 10. `wiki extract`

```bash
echo "See [[Authentication]] and [[Sessions]] for context." | wiki extract
```

Expect: each wikilink's title + summary, one block per link.

## 11. `wiki check`

```bash
wiki check ; echo "exit:$?"
```

Expect: validates frontmatter and wikilinks across all `*.md` and `*.wiki.md`. With the seed content above, exit `0` and no errors.

### 11a. inject a broken link, observe failure

```bash
echo "Broken: [[NoSuchPage]]" >> docs/sessions.md
wiki check ; echo "exit:$?"
```

Expect: error pointing at `docs/sessions.md` referencing an unresolved title. Exit non-zero.

### 11b. cross-namespace broken reference

```bash
sed -i 's/scratch:Operator Notes/scratch:Missing/' docs/authentication.md
wiki check ; echo "exit:$?"
```

Expect: validation error from rule 6 (undeclared/missing cross-namespace article).

### 11c. revert

```bash
git checkout -- docs/sessions.md docs/authentication.md
wiki check ; echo "exit:$?"
```

Expect: clean again, exit `0`.

### 11d. JSON

```bash
wiki check --format json
```

Expect: structured JSON report (empty `errors` array on a clean wiki).

## 12. `wiki hook` (PostToolUse handler)

```bash
printf '{"tool_input":{"file_path":"%s/docs/authentication.md"}}' "$WORK" | wiki hook ; echo "exit:$?"
```

Expect: exit `0` and no `systemMessage` because the file is clean. With a broken edit (e.g. introducing `[[BadLink]]`), the hook prints a JSON `{"systemMessage":"…"}` describing the failure.

## 13. `wiki scaffold` (git mesh integration)

```bash
wiki scaffold > /tmp/scaffold.md
head -20 /tmp/scaffold.md
```

Expect: a markdown document with one section per fragment-link group — each section carries the source heading, the opening sentence as a blockquote, and a fenced bash block of `git mesh add <slug> <anchor>` / `git mesh why <slug> -m "[why]"` commands. A trailing "Commit Changes After Review" block lists every `git mesh commit` line. The document is safe to inspect; copying the bash blocks into your shell stages mesh data (do this only if you have `git mesh` installed).

When the wiki has no uncovered fragment links, the output is a single-paragraph markdown notice (`# wiki scaffold` + "No uncovered fragment links — every link is already covered by a mesh."), not the document above.

## 14. `wiki install`

```bash
wiki install --help
```

Expect: subcommands listing supported integration targets (e.g. `claude`, `gemini`). `wiki install <target>` writes the integration config; verify by inspecting the target's config dir afterwards. Skip in this throwaway repo unless you also want to test the integration.

## 15. `--perf` and exit-code conventions

```bash
WIKI_PERF=1 wiki "Authentication" 2>/tmp/perf.log >/dev/null
head -5 /tmp/perf.log
```

Expect: per-event timing lines on stderr; query result on stdout.

Exit-code summary across the suite:

| Outcome | Exit |
|---|---|
| Success / clean check | `0` |
| Soft validation problem (e.g. duplicate ns rows still printed) | `1` |
| Hard config / arg failure (no `wiki.toml`, unknown namespace, init refusal) | `2` |

## 16. Cleanup

```bash
cd /
rm -rf "$WORK"
```

## Coverage matrix

| Feature | Step |
|---|---|
| `init` (default, named, refusal, validation) | 2 |
| `namespaces` (text, JSON, duplicate, parse error) | 3, 3a–3c |
| Multi-namespace authoring (default + `scratch` + `*.wiki.md`) | 4 |
| `list` (single, peer, `*`) | 5 |
| Search (default, `-n`, `@`-sugar, `*`, unknown, JSON, `-l`, `-o`) | 6 |
| `summary`, `refs`, `links`, `extract` | 7–10 |
| `check` (clean, broken link, cross-ns, JSON) | 11 |
| `hook` (PostToolUse JSON in/out) | 12 |
| `scaffold` | 13 |
| `install` | 14 |
| `--perf` and exit-code conventions | 15 |
