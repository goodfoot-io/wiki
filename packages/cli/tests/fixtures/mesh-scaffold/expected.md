# Charge handler notes • src/notes.wiki.md

## Charge handler notes
> Implementation lives in handleCharge.

```bash
git mesh add src/charge-handler-notes \
  src/notes.wiki.md \
  src/charge.ts#L2-L7
git mesh why src/charge-handler-notes -m "[why]"
```

# Billing • wiki/billing.md

## Billing
> The billing service validates the checkout payload before submitCheckout is called.

```bash
git mesh add wiki/billing \
  wiki/billing.md \
  src/checkout.ts#L2-L8
git mesh why wiki/billing -m "[why]"
```

## Charge handler
> The server-side handler handleCharge validates the schema and dispatches to Stripe.

```bash
git mesh add wiki/charge-handler \
  wiki/billing.md \
  src/charge.ts#L2-L7
git mesh why wiki/charge-handler -m "[why]"
```

# CLI parser • wiki/cli/parser.md

## CLI parser
> The argument parser entrypoint is parse_args.

```bash
git mesh add wiki/cli/cli-parser \
  wiki/cli/parser.md \
  src/parser.rs#L2-L4
git mesh why wiki/cli/cli-parser -m "[why]"
```

# Edge cases • wiki/edge.md

## git-mesh ls
> The command git_mesh_ls lists meshes touching an anchor.

```bash
git mesh add wiki/git-mesh-ls \
  wiki/edge.md \
  src/parser.rs#L2-L4
git mesh why wiki/git-mesh-ls -m "[why]"
```

## Identifier predicate
> build_index is the entry point used by build_index.

```bash
git mesh add wiki/identifier-predicate \
  wiki/edge.md \
  src/index.rs#L10-L20
git mesh why wiki/identifier-predicate -m "[why]"
```

## Bold label only
> where_anchor

```bash
git mesh add wiki/bold-label-only \
  wiki/edge.md \
  src/index.rs#L25-L40
git mesh why wiki/bold-label-only -m "[why]"
```

## Table opening
> After the table the table_anchor is referenced once more.

```bash
git mesh add wiki/table-opening \
  wiki/edge.md \
  src/index.rs#L45-L60
git mesh why wiki/table-opening -m "[why]"
```

## Table opening
> After the table the table_anchor is referenced once more.

```bash
git mesh add wiki/table-opening-2 \
  wiki/edge.md \
  src/checkout.ts#L2-L8
git mesh why wiki/table-opening-2 -m "[why]"
```

## Ordered list opening
> Validates the schema before validate_step is invoked.

```bash
git mesh add wiki/ordered-list-opening \
  wiki/edge.md \
  src/charge.ts#L2-L7
git mesh why wiki/ordered-list-opening -m "[why]"
```

## Truly degenerate
> 1.

```bash
git mesh add wiki/truly-degenerate \
  wiki/edge.md \
  src/index.rs#L70-L80
git mesh why wiki/truly-degenerate -m "[why]"
```

# Incremental indexing • wiki/perf/indexing.md

## (top of file)
> See bootstrap for the entry point.

```bash
git mesh add wiki/perf/bootstrap \
  wiki/perf/indexing.md \
  src/index.rs#L1-L5
git mesh why wiki/perf/bootstrap -m "[why]"
```

## Sync detection
> The WikiIndex sync detects changes incrementally.

```bash
git mesh add wiki/perf/sync-detection \
  wiki/perf/indexing.md \
  src/index.rs#L10-L20
git mesh why wiki/perf/sync-detection -m "[why]"
```

## Apply phase
> The indexer applies each diff entry to the in-memory tree; the implementation spans apply_changes and apply_changes_batch.

```bash
git mesh add wiki/perf/apply-phase \
  wiki/perf/indexing.md \
  src/index.rs#L25-L40
git mesh why wiki/perf/apply-phase -m "[why]"
```

## Apply phase
> The indexer applies each diff entry to the in-memory tree; the implementation spans apply_changes and apply_changes_batch.

```bash
git mesh add wiki/perf/apply-phase-2 \
  wiki/perf/indexing.md \
  src/index.rs#L45-L60
git mesh why wiki/perf/apply-phase-2 -m "[why]"
```

## Cache layer
> The cache wiki section describes the LRU cache used by index lookups, backed by CacheKey.

```bash
git mesh add wiki/perf/cache-layer \
  wiki/perf/indexing.md \
  src/index.rs#L70-L80
git mesh why wiki/perf/cache-layer -m "[why]"
```

