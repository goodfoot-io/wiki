# Fragment links and the coverage contract

A fragment link points from a page to a sibling source file. It is the unit of drift detection — so **always include a line range**.

```markdown
The retry loop lives in [client.ts](../packages/api/client.ts#L88-L120).
```

Whole-file links (`[client.ts](../packages/api/client.ts)`) are valid but discouraged: coverage falls back to the `0-0` sentinel and you lose line-level drift signal. Use a range whenever the cited thing has one.

Path resolution follows the page-link rules in `./writing-a-page.md` (`http(s)` links are unvalidated and uncovered).

## Fragment link discipline

When prose mentions a code component, link it — and link it well:

- **If you mention it, link it.** Every function, type, schema, constant, or module named in prose gets a fragment link to its definition.
- **Link definitions, not call sites.** Target the signature/declaration, not where it's invoked.
- **Span the whole definition** — opening line through closing brace.
- **Orphans go in `## References`.** A component that resists natural prose placement still gets its link, collected at the page bottom.

## The coverage contract (non-obvious)

For every fragment link `path#L<start>-L<end>`, there must exist **one** mesh under `.wiki/` that anchors **both**:

1. the **code target** — at exactly `start-end`, or as a whole-file `0-0` anchor, **and**
2. the **wiki page itself**.

A mesh anchoring only one side does **not** cover the link. This is the rule operators miss most. Links without a line range and external links are exempt.

You rarely create this coverage by hand — `wiki check --fix` and the pre-commit hook build it for you (see `./fixing-mesh-coverage.md`). Hand-create it only when `--fix` skips (see `./resolving-skipped-fixes.md`), and when you do, remember: **one `wiki mesh add` must name both the page and the code anchor.**

The mesh stores the rk64 hash of the anchored bytes, not the text. When those bytes change, the anchor goes stale and `wiki check` flags the link — that staleness *is* the "this article may be wrong" signal.
