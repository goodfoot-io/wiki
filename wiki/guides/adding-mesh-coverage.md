---
title: Adding Mesh Coverage
summary: Workflow for establishing baseline git mesh coverage across uncovered wiki pages — scaffolding, rewriting whys to be idiomatic, consolidating anchors, and pitfalls to avoid.
tags:
  - wiki
  - git-mesh
  - guide
---

This guide captures the technique for taking a wiki repository from "many uncovered fragment links" to "every fragment link is covered by a mesh whose `why` reads like a definition." It is meant for an agent or operator establishing baseline coverage, not for incremental coverage during normal authoring.

For the design behind the integration, see [[Wiki Mesh Integration]]. For the command catalog, see [[git-mesh Usage in the Wiki CLI]]. For the canonical rules on naming and writing whys, the `git-mesh:handbook` skill is the source of truth — load it first.

## 1. Inventory the gap

```bash
wiki check
```

Each `mesh_uncovered` finding names the wiki file, the fragment link, and the line range that lacks a covering mesh. Skim the list before scripting anything: it tells you which subsystems will dominate the work, and it reveals duplicates (the same anchor appearing in two sections of the same page).

## 2. Scaffold per page

```bash
wiki scaffold wiki/architecture/wiki-cli.md
```

The output is a markdown document — one section per fragment link, each ending in a fenced bash block of `git mesh add` and `git mesh why` commands, plus a trailing "Commit Changes After Review" block that lists every `git mesh commit` line. The whole document is **a starting point, not a finished artifact**. The scaffold derives:

- **Mesh names** from the page title slug and the link label.
- **Whys** from the prose sentence that contains the link, with markdown stripped.

Both are routinely wrong in ways the handbook calls out:

- Headless predicates ("Validation — the validation pipeline wiki section describes validation in check.") because the source sentence opened with a backtick identifier.
- Diff-style restatements ("the X wiki section describes Y in Z") that name the *coupling* rather than the *subsystem*.
- Generic slugs (`wiki/cli/command-1`, `wiki/files`) that won't survive a rename of either side.

Treat scaffold output the way you would treat machine-generated commit messages: useful priors, not finished prose.

## 3. Rewrite the why as a definition

The handbook's rule is one sentence:

> Write the why as a definition: name the subsystem, flow, or concern the anchors collectively form, and say plainly what it does across them.

**Read the wiki section first; write the why from the page's framing — not from the link's surrounding sentence and not from the scaffold's draft.** The wiki page already chose a heading and an opening paragraph that name the subsystem; that framing is almost always closer to a good why than either the scaffold's auto-extraction or the inline sentence containing the fragment link. Open the file, locate the heading the link sits under, read the first sentence after it, and write the definition from there.

Concrete techniques that produced idiomatic whys on this pass:

- **Lead with a noun phrase that names the subsystem**, not a verb describing what the doc does. "Validation pipeline that drives…" beats "The check command validates…".
- **State the scope across both anchors in one clause.** A doc-to-code mesh says what the subsystem *is*, not "the doc describes the code". The anchors and the slug already record the directionality.
- **Drop scolds, ownership, and review triggers.** "Don't change X without Y", "Owner: team-foo", "Review on body changes" all belong elsewhere — code comments, CODEOWNERS, PR templates.
- **Keep it evergreen.** A future reader who has never seen the diff should be able to read the why and learn what the subsystem is. If the why depends on the current state of the diff, it will rot.
- **Mirror the page's framing without restating its prose.** The wiki page already has a section heading and explanatory paragraph; the why should be the same idea compressed to one sentence and freed from the page's voice.

Worked examples from this baseline pass:

| Scaffold draft | Rewritten why |
|---|---|
| "Validation — the validation pipeline wiki section describes validation in check." | "End-to-end validation pass that drives frontmatter parsing, title/alias collision detection, wikilink resolution, and fragment-link verification for every wiki page." |
| "Dir — the default glob behavior wiki section describes default in mod." | "Default file-discovery contract shared by every wiki command: when no globs are passed, the CLI walks `$WIKI_DIR/**/*.md` plus `**/*.wiki.md`, and the mesh-integration design page promises that same default." |
| "Confirm contract that synchronizes the pages shape expected by the update order wiki section with what card files table provides." | "Wiki skill workflow contract that defines page-discovery, location, validation, and cross-linking steps for agents; the touchpoints register holds it in sync with the CLI it drives." |

## 4. Consolidate anchors that share a relationship

The scaffold emits one mesh per uncovered link. The handbook says **one relationship per mesh** — not one anchor per mesh. When two or more anchors form a single subsystem, fold them into a single `git mesh add` call:

```bash
# Two scaffold lines about incremental WikiIndex sync — one subsystem, one mesh
git mesh add wiki/perf/incremental-indexing \
  wiki/guides/wiki-performance-optimization.md \
  packages/cli/src/index.rs#L945-L945 \
  packages/cli/src/index.rs#L960-L960
git mesh why wiki/perf/incremental-indexing -m "Incremental WikiIndex sync that detects changes by probing Git state (HEAD SHA, wiki dir, working-tree status) so only added, modified, or deleted pages are re-parsed."
```

Same applies when one scaffolded relationship appears twice on the same page (e.g. once in a "what to update" section and again in an "update order" checklist) — collapse to one mesh per unique anchor.

Conversely, if the scaffold produced one mesh whose anchors actually serve two reasons-to-change-together, split it.

## 5. Rename to a relationship slug

Avoid scaffold defaults like `wiki/cli/command-1` or `wiki/files`. Pick a kebab-case noun phrase that survives a rewrite of either side and a category prefix that locates it in the repo's vocabulary (`wiki/perf/`, `wiki/touchpoints/`, `wiki/mesh-usage/`, `wiki/resolution/`). The slug should describe the *thing the anchors form together*, not either anchor individually.

## 6. Commit each mesh

```bash
git mesh commit wiki/perf/incremental-indexing
```

A first-time commit fails if no `why` is staged — that's the normal flow. Errors of the form `error: path not in tree: <file> at <sha>` mean the anchored file is staged but not present in HEAD; commit (or stash) the file first, then retry.

## 7. Verify

```bash
wiki check
```

Expect zero `mesh_uncovered` findings on real wiki pages. Test fixtures that intentionally exercise the uncovered code path (e.g. `packages/cli/tests/fixtures/mesh-scaffold/`) will keep showing up — leave them.

## Things worth knowing before you start

- **The scaffold's whys are diff-shaped, not subsystem-shaped.** Plan to rewrite every one. Skimming and approving as-is produces meshes that read like commit messages and rot the same way.
- **Duplicate anchors on the same page are a signal, not an error.** Both findings should be one mesh; the page itself is structured to point at the same code from two angles (e.g. "what" + "update order").
- **Multiple anchors can belong to one mesh.** Resist the scaffold's one-mesh-per-anchor framing whenever the relationship is single.
- **Whole-file anchors are the recommended default for prose meshes.** This baseline pass kept line-range anchors because the wiki pages already used them and the targets are stable, but expect editorial drift on prose to push some of them toward whole-file form. See `responding-to-drift.md` in the git-mesh handbook.
- **`git mesh add` resolves anchors against HEAD at commit time.** A newly authored wiki page must be committed before its meshes can commit. Stage the file and create a normal commit first; mesh commits will then succeed.
- **Mesh commits and source commits are independent.** You can establish baseline coverage as a standalone exercise — no source change is required, no source commit is produced.
- **Don't mesh test fixtures.** Fixtures that intentionally simulate "uncovered" state to exercise the integration are not a coverage gap; meshing them defeats the test.
- **The handbook is the authority on naming and whys.** When in doubt, re-read `creating-a-mesh.md` from the git-mesh handbook skill rather than guessing.

## References

- [[Wiki Mesh Integration]]
- [[git-mesh Usage in the Wiki CLI]]
- [[Wiki CLI]]
