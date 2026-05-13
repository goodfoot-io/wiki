# Wiki Maintenance

Step-by-step instructions for identifying drifted wiki pages, updating them, and committing the results.

---

## 1. Identify Drifted Anchors

```bash
git mesh stale
```

- **No drifted anchors**: Run `wiki check` to confirm the wiki is clean, then stop.
- **Drifted anchors found**: Continue to Step 2.

---

## 2. Prioritize

Group drifted anchors by mesh. For each mesh, run:

```bash
git mesh <slug>
```

Review which wiki file and source file are anchored together. Pages whose anchors touch widely-used code or whose wikilinks are referenced from many places should be processed first.

---

## 3. For Each Drifted Mesh

### 3.1 Get the Diff

```bash
git mesh stale <slug>
```

Read every drifted anchor's diff in full before making any changes. Understand all changes before deciding what prose to update.

### 3.2 Classify Each Change

For each drifted anchor, determine the update path:

- **Behavior changed** (new parameters, new logic, deleted functionality, changed return values): Update the prose to reflect the new behavior, then continue to 3.3
- **Cosmetic only** (variable rename, reformatted output, reorganized tests, added imports): Confirm prose remains accurate; re-anchor without prose changes
- **Code deleted**: Remove the fragment link and the prose that referenced it; verify the page is still coherent
- **Ambiguous**: Make a judgment call; record the entry as uncertain in the Step 6 summary

The deciding question: *does the diff change what the code does, or only how it looks?*

### 3.3 Fragment Link Discipline

When updating prose, apply this discipline to every component you mention:

- **If you mention it, link it.** Any function, type, schema, constant, or module introduced into the prose must have a corresponding fragment link pointing to its definition.
- **Link to definitions, not call sites.** Target the function signature, type definition, or schema declaration — not where it is invoked.
- **Use broad ranges.** Span from the opening line of the definition through its closing brace.
- **Use a References section for orphaned links.** If a component resists natural prose placement, collect its fragment link in a `## References` section at the bottom of the page.

### 3.4 Update Cross-References

After updating prose, check whether any relative markdown links on the page point to pages that have themselves become inaccurate as a result of the same changes. Update those linked pages as needed before moving on.

### 3.5 Re-anchor

After all prose updates for the page are complete, stage updated anchors for every fragment link that changed:

```bash
git mesh add <slug> <wiki-file> <source-anchor>   # e.g. packages/cli/src/foo.rs#L10-L40
```

Then commit mesh data (or let the post-commit hook do it):

```bash
git mesh commit
```

### 3.6 Check Cross-References

Run:

```bash
wiki links "Updated Page Title"
```

Read each page that links **to** the updated page. If the updated page's behavior is now described differently, check whether the linking page's prose still holds. Update and re-anchor any that need it.

---

## 4. Cover New Fragment Links

After all prose edits:

```bash
wiki scaffold
```

Review the output. If any fragment links are uncovered, run the generated `git mesh add` and `git mesh why` commands, then commit:

```bash
git mesh commit
```

---

## 5. Verify Clean

```bash
wiki check
```

Expect: no errors.

- **Clean**: Proceed to Step 6.
- **Errors remain**: Return to Step 3 for any pages still flagged.

---

## 6. Commit

Stage all modified wiki pages and commit:

```bash
git add wiki/ documentation/
git commit -m "wiki: maintenance pass — [brief summary of what changed]"
```

Write the commit message body as a short list of what changed and why, one line per page updated. Example:

```
wiki: maintenance pass — update cards and rebuild pages

- cards-extension: updated issue lifecycle section after HybridStore refactor
- rebuild: re-anchored only, cosmetic rename in rebuild.ts
- card-files-table: removed stale link to deleted migration helper
```

---

## 7. End-of-Pass Summary

Report:

- **Pages updated** — pages where prose changed; one line per page describing what changed and why
- **Re-pinned only** — pages where only line ranges were adjusted (no prose changes needed)
- **Flagged (uncertain)** — entries where the diff was ambiguous; describe the judgment call made
- **New meshes created** — meshes added by `wiki scaffold` for previously uncovered fragment links
