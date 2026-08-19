# Fragment links and the anchor contract

A fragment link points from a page to a sibling source file. It is the unit of drift detection — so **always include a line range**.

```markdown
The retry loop lives in [client.ts](../packages/api/client.ts#L88-L120).
```

Whole-file links (`[client.ts](../packages/api/client.ts)`) are valid but discouraged: no range, no drift signal. Use a range whenever the cited thing has one.

Path resolution follows the page-link rules in `./writing-a-page.md` (`http(s)` links are unvalidated).

## Fragment link discipline

When prose mentions a code component, link it — and link it well:

- **If you mention it, link it.** Every function, type, schema, constant, or module named in prose gets a fragment link to its definition.
- **Link definitions, not call sites.** Target the signature/declaration, not where it's invoked.
- **Span the whole definition** — opening line through closing brace.
- **Orphans go in `## References`.** A component that resists natural prose placement still gets its link, collected at the page bottom.

## The anchor contract (non-obvious)

Every page carrying line-range links is certified by a single frontmatter field:

```yaml
links-reviewed: 1
```

The field's value is the page's **anchor epoch**. The engine walks the page's git history to the commit where that value last changed — the **anchor commit** — and takes the cited files *as they were at that commit* as the certified baseline. Each link is then classified against that baseline:

- **Healthy** — the cited range's content is byte-identical to the baseline.
- **Drift** — same place, different bytes. *This article may now be wrong.* The remedy is reading the diff and bumping the field (whole-page re-certification) — `--fix` will not do it for you.
- **Moved** — the certified bytes now live elsewhere; `--fix` rewrites the link to follow them.
- **Broken** — the target is gone or the range no longer fits; `--fix` routes it through the rename machinery, and skips when no unambiguous move exists.
- **Uncertified** — the href points somewhere the page never reviewed: the as-written path and range aren't among the certified links, and the bytes there aren't the certified block.
- **Fail-closed** — the page has no `links-reviewed:` field at all (the check errors; `--fix` initializes the field), the anchor commit can't be resolved in a shallow clone, or the certified content matches multiple locations.

One field certifies **every** line-range link on the page together: bumping it is a whole-page claim, so bump only after reviewing every drifted link on the page (see `./resolving-skipped-fixes.md`).

The baseline stores the rk64 hash of the cited bytes, not the text. When those bytes change after the anchor commit, `wiki check` flags the link — that staleness *is* the "this article may be wrong" signal.
