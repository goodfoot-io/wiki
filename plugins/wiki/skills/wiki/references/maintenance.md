# Wiki Maintenance

Step-by-step instructions for identifying stale wiki pages, updating them, and committing the results.

---

## 1. Identify Stale Pages

```bash
wiki stale
```

- **No stale links**: Run `wiki check --fix` to pin any unpinned links on new pages, then stop.
- **Stale links found**: Continue to Step 2.

---

## 2. Prioritize

Group stale links by page. For each page, check its backlink count:

```bash
wiki backlinks "Page Title"
```

Process pages with the most backlinks first — they have the widest impact on wiki coherence when out of date.

---

## 3. For Each Stale Page

### 3.1 Get the Full Diff

```bash
wiki stale --diff patch 'wiki/path/to/page.md'
```

Read every stale link's diff in full before making any changes. Understand all changes on the page before deciding what prose to update.

### 3.2 Classify Each Change

For each stale link, determine the update path:

- **Behavior changed** (new parameters, new logic, deleted functionality, changed return values): Update the prose to reflect the new behavior, then continue to 3.3
- **Cosmetic only** (variable rename, reformatted output, reorganized tests, added imports): Confirm prose remains accurate; re-pin without prose changes
- **Code deleted**: Remove the fragment link and the prose that referenced it; verify the page is still coherent
- **Ambiguous**: Make a judgment call; record the entry as uncertain in the Step 6 summary

The deciding question: *does the diff change what the code does, or only how it looks?*

### 3.3 Fragment Link Discipline

When updating prose, apply this discipline to every component you mention:

- **If you mention it, link it.** Any function, type, schema, constant, or module introduced into the prose must have a corresponding fragment link pointing to its definition. Prose without a link is a blind spot — when the component changes, the page will not appear in `wiki stale` output.
- **Link to definitions, not call sites.** Target the function signature, type definition, or schema declaration — not where it is invoked.
- **Use broad ranges.** Span from the opening line of the definition through its closing brace. A narrow range that excludes the body produces false confidence: when the implementation changes, the page should go stale so the prose can be reviewed.
- **Use a References section for orphaned links.** If a component resists natural prose placement, collect its fragment link in a `## References` section at the bottom of the page rather than forcing awkward prose.

After adding new fragment links, run `wiki check --fix` to pin them before re-pinning the rest of the page.

### 3.4 Update Wikilinks

After updating prose, check whether any wikilinks on the page point to pages that have themselves become stale or inaccurate as a result of the same changes. Update wikilinks and linked pages as needed before moving on.

### 3.5 Re-pin the Page

After all prose updates for the page are complete:

```bash
wiki pin
```

`wiki pin` re-pins all existing SHAs across all pages to HEAD. Run it once per page pass, not after each individual link.

### 3.6 Check Backlinks

```bash
wiki backlinks "Updated Page Title"
```

Read each backlinking page. If the updated page's behavior is now described differently, check whether the backlinking page's prose still holds. Update and re-pin any that need it, then check their backlinks in turn.

---

## 4. Final Cleanup

```bash
wiki check --fix
```

Pins any unpinned fragment links (e.g. on pages created since the last maintenance pass) and validates the full wiki. This catches issues `wiki stale` does not cover.

---

## 5. Verify Clean

```bash
wiki stale
```

Expect: `No stale links found.`

- **Clean**: Proceed to Step 6.
- **Remaining stale links**: Return to Step 3 for those pages.

---

## 6. Commit

Stage all modified wiki pages and commit:

```bash
git add wiki/ documentation/**/*.wiki.md
git commit -m "wiki: maintenance pass — [brief summary of what changed]"
```

Write the commit message body as a short list of what changed and why, one line per page updated. Example:

```
wiki: maintenance pass — update cards and rebuild pages

- cards-extension: updated issue lifecycle section after HybridStore refactor
- rebuild: re-pinned only, cosmetic rename in rebuild.ts
- card-files-table: removed stale link to deleted migration helper
```

---

## 7. End-of-Pass Summary

Report:

- **Pages updated** — pages where prose changed; one line per page describing what changed and why
- **Re-pinned only** — pages where only SHAs were advanced (no prose changes needed)
- **Flagged (uncertain)** — entries where the diff was ambiguous; describe the judgment call made
- **Other fixes** — unpinned links pinned by `wiki check --fix`
