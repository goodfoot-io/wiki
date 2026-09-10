# Reconcile flagged wiki links

Four stages. Stages 1, 2, and 4 belong to whoever owns the reconciliation — run
them once, yourself. Stage 3 is per-page work, divisible by component and the
only stage that may be delegated (see `./team.md`).

**Report** below means your final summary if you own the reconciliation, or your
message to the owner if you are a delegated worker.

Invoke `wiki` only when a topic exceeds what is here; navigate to
the named section.

---

## Stage 1 — Prelude

The only mutating research step.

```bash
wiki check --fix
```

This relocates moved links, routes broken targets through renames, and
initializes `links-reviewed:` on any page that lacks it. It never resolves a
link it cannot relocate with confidence, and a page that already carries
`links-reviewed:` is never bumped for you — see
[When --fix skips](../../wiki/how-to/validate-and-fix.md#when---fix-skips).

Remaining findings are `link_drift`, `link_broken`, `link_uncertified`,
`link_unverified`, and any "moved but not byte-identical" link `--fix`
reported instead of resolving. `frontmatter` and `collision` diagnostics are
page-authoring problems, not link drift — out of scope here.

Group what remains by **page** first: certification is per page —
`links-reviewed:` covers every line-range link on it together — so every
flagged link on the same page is already one unit of work with one
commit-worthy bump. If nothing remains, you are done — skip stages 2–4.

---

## Stage 2 — Partition

`wiki check --format json` lists every flagged link as `{kind, file, line,
message}` — `file` is the page, not the cited target. Read each flagged
line in its page to recover the cited target (`path#Lstart-Lend`) from the
href itself; there is no equivalent of a dependency-tree command to do this
for you.

Group the flagged **pages** into components: two pages belong to the same
component if any of their flagged links cite the same target file. Reading
that target's history once (`git log -L <start>,<end>:<file>`, or a blame)
settles the classification for every page citing it, so treating them as one
unit avoids re-deriving the same fact per page. A page whose flagged links
each cite a target no other flagged page cites is a component of size one.

Record per component: every page; every flagged link on it (target, range,
kind); the shared target files driving the grouping; and a flag on any
targets whose cited ranges overlap across pages in the component. This is
structural — what is flagged, not why it drifted.

Never separate two pages of the same component across delegated units or
sub-batches. Split an oversized component into cohesive sub-batches by topic
or subsystem instead.

---

## Stage 3 — Per-page procedure

Per page, in each component. Resolve every flagged link on the page before
touching `links-reviewed:` — a partial bump certifies links nobody reviewed.

Follow [When --fix skips](../../wiki/how-to/validate-and-fix.md#when---fix-skips)
for each flagged link: read the cited range's history, read the page's prose
around the link, decide whether the change is behavioral or cosmetic, and act
— update the prose, accept the relocation, hand-edit the fragment, or drop the
link. Within a component, read a shared target's history once and apply the
same behavioral-vs-cosmetic call to every page citing it; only the
prose-accuracy judgment stays per page.

Never batch-bump `links-reviewed:` to clear an exit code, and never bump
before every flagged link on that page is resolved. Once every flagged link on
the page is resolved:

```yaml
links-reviewed: 2   # was 1
```

Then, scoped to that page:

```bash
wiki check <page>   # must exit 0 with nothing left flagged
```

If your prose edit makes a linked page inaccurate too, fix that page before
moving on — it may open flagged links of its own that your check hasn't seen
yet.

### Deciding authority

A page drifting behind a deliberate, committed code change is wrong — conform
it. A code change with no coherent commit story may itself be the regression
— the page may be the truth. Fail closed on ambiguity: stop and report rather
than bumping `links-reviewed:` over a live disagreement.

---

## Stage 4 — Validation and commit — owner only

```bash
wiki check     # must exit 0 across everything you touched
```

Run required validation for every behavior change and surface every link
diff. If validation fails, handle the failing component before committing.

```bash
git add <changed-pages>
git commit -m "Reconcile flagged wiki links"
```

---

## Allowlist

Restrict yourself to: `wiki check` in any form; edits to a flagged page's
prose, its link hrefs, and its own `links-reviewed:` field; required tests;
and read-only `git log`/`git diff`/`git show`/`git blame`. Never touch
unrelated paths.

Staging and committing are stage 4, so they are the owner's alone. A
delegated worker commits nothing and leaves its page edits unstaged for the
owner.
