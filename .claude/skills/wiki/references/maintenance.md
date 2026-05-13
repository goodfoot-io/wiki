# Wiki Maintenance

Step-by-step instructions for reviewing wiki pages for accuracy, updating them, and committing the results.

---

## 1. Identify Pages to Review

There is no automated staleness detector. Use these signals to find pages that may need attention:

- **`wiki check` failures** — broken file paths, out-of-bounds line ranges, or missing SHAs
- **Recent code changes** — run `git log --oneline -20` and find wiki pages that link to changed files via `wiki links <path>`
- **Broken fragment links** — if a linked file was deleted or moved, `wiki check` will report `missing_file`

```bash
wiki check
```

- **No errors**: Run `wiki check --fix` to pin any unpinned links on new pages, then stop.
- **Errors found**: Continue to Step 2.

---

## 2. Prioritize

Group errors by page. For each page, check which other pages link to it:

```bash
wiki links "Page Title"
```

Process pages with the most incoming links first — they have the widest impact on wiki coherence when out of date.

---

## 3. For Each Page

### 3.1 Review the Linked Code

Open the fragment-linked source files and compare them to the prose. Check whether the documented behavior still matches:

- **Behavior changed** (new parameters, new logic, deleted functionality, changed return values): Update the prose to reflect the new behavior, then continue to 3.2
- **Cosmetic only** (variable rename, reformatted output, reorganized tests, added imports): Confirm prose remains accurate; update fragment link line ranges if needed
- **Code deleted**: Remove the fragment link and the prose that referenced it; verify the page is still coherent
- **Ambiguous**: Make a judgment call; record the entry as uncertain in the Step 5 summary

The deciding question: *does the change affect what the code does, or only how it looks?*

### 3.2 Fragment Link Discipline

When updating prose, apply this discipline to every component you mention:

- **If you mention it, link it.** Any function, type, schema, constant, or module introduced into the prose must have a corresponding fragment link pointing to its definition. Prose without a link is a blind spot — when the component changes, the page has no signal.
- **Link to definitions, not call sites.** Target the function signature, type definition, or schema declaration — not where it is invoked.
- **Use broad ranges.** Span from the opening line of the definition through its closing brace. A narrow range that excludes the body misses implementation changes.
- **Use a References section for orphaned links.** If a component resists natural prose placement, collect its fragment link in a `## References` section at the bottom of the page rather than forcing awkward prose.

After adding new fragment links, run `wiki check --fix` to pin them.

### 3.3 Update Wikilinks

After updating prose, check whether any wikilinks on the page point to pages that have themselves become inaccurate as a result of the same changes. Update wikilinks and linked pages as needed before moving on.

### 3.4 Validate and Pin

After all prose updates for the page are complete:

```bash
wiki check --fix wiki/path/to/page.md
```

### 3.5 Check Incoming Links

```bash
wiki links "Updated Page Title"
```

Read each linking page. If the updated page's behavior is now described differently, check whether the linking page's prose still holds. Update and re-validate any that need it.

---

## 4. Final Cleanup

```bash
wiki check --fix
```

Pins any unpinned fragment links (e.g. on pages created during this pass) and validates the full wiki.

---

## 5. Verify Clean

```bash
wiki check
```

Expect: no output, exit 0.

- **Clean**: Proceed to Step 6.
- **Remaining errors**: Return to Step 3 for those pages.

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
- rebuild: re-pinned only, cosmetic rename in rebuild.ts
- card-files-table: removed stale link to deleted migration helper
```

---

## 7. End-of-Pass Summary

Report:

- **Pages updated** — pages where prose changed; one line per page describing what changed and why
- **Re-pinned only** — pages where only line ranges were adjusted (no prose changes needed)
- **Flagged (uncertain)** — entries where the change was ambiguous; describe the judgment call made
- **Other fixes** — unpinned links pinned by `wiki check --fix`
