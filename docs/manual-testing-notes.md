# Manual Testing Notes

Single linear pass through `wiki/guides/manual-testing-procedure.md` against
`wiki 0.5.38` (release build of the current tree). Records divergences from the
procedure and counter-intuitive behavior. Steps not listed here behaved exactly
as documented.

## Bugs / non-spec behavior

### `wiki install` surface mismatches the procedure (Step 14)

Procedure: "subcommands listing supported integration targets (e.g. `claude`,
`gemini`). `wiki install <target>` writes the integration config".

Actual:

- Flag-based: `wiki install --codex` and `wiki install --claude`.
- `--claude` is informational only (prints setup instructions; touches
  nothing).
- `gemini` is not a target.
- Help text does mention `--codex` does network I/O and writes managed
  files — that part is well-documented in `--help`.

The procedure's example wording (`wiki install <target>`) is not a real
invocation pattern.

## Counter-intuitive behavior

### `wiki "Authentication" -l 1 -o 1` returns silently empty (Step 6b)

Only one match exists for `Authentication`, so `-o 1` skips past every
result. The CLI emits no output and exits `0` — no "no more results" hint, no
"showing 0 of 1" footer. The reader of the procedure ("page 1 then page 2 of
results") will reasonably expect a non-empty page 2 and may not realize the
silent exit is the intended pagination signal.

### `wiki summary` and `wiki "<query>"` produce visually similar output

`wiki summary "Authentication"` and `wiki "Authentication"` both print
`# Title` / `## path` / summary; the search version adds a `Matched snippets:`
block, the summary version does not. Different commands, similar headers,
not visually distinct enough that a copy-pasted log clearly identifies which
command produced it.

### `wiki scaffold` empty output exercises only the empty-corpus path (Step 13)

With seed content that contains wikilinks but no fragment-link anchors, the
output is the empty-corpus markdown notice:

```markdown
# wiki scaffold

No uncovered fragment links — every link is already covered by a mesh.
```

That is the intended empty case, but the procedure body now describes the
non-empty markdown document (per-section headings, blockquote opening
sentences, fenced bash blocks, and a trailing "Commit Changes After Review"
block) without explicitly seeding fragment-anchor content beforehand. As
written, Step 13 exercises only the empty-output path. The non-empty branch
is covered by `packages/cli/tests/fixtures/mesh-scaffold/expected.md` but is
never reached by the manual procedure.

### `--format json` clean check returns `[]`, not a structured report (Step 11d)

Procedure: "structured JSON report (empty `errors` array on a clean wiki)".
Actual: a bare empty JSON array `[]`. There is no envelope object with a
named `errors` key — the array *is* the error list. Minor wording mismatch in
the procedure.

### `wiki -n '*'` allowed surface is still discovered by trial-and-error

The set of subcommands that accept `-n '*'` (search, check, links, summary,
refs) is not surfaced in `wiki --help` or in per-subcommand help. A user who
tries `wiki -n '*' list` learns the answer only via the rejection error
message. Documented in the procedure's coverage matrix but not in the CLI
itself.

## Procedure hygiene

### Coverage matrix references removed sections

`wiki/guides/manual-testing-procedure.md` ends with a coverage matrix that
still lists:

```
| `html`, `serve` | 13–14 |
```

These subcommands have been removed; the matrix row should be deleted, and
the `Step` column for the rows below it should be renumbered to match the
current section numbers (`scaffold` is now Step 13, `install` Step 14, `--perf`
Step 15).
