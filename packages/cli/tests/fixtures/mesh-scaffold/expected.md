# Charge handler notes • src/notes.wiki.md

> Implementation lives in [handleCharge](./charge.ts#L2-L7).

```bash
git mesh add src/charge-handler-notes \
  src/notes.wiki.md \
  src/charge.ts#L2-L7
git mesh why src/charge-handler-notes -m "[why]"
```

---

# Billing • wiki/billing.md

> The billing service validates the checkout payload before [submitCheckout](src/checkout.ts#L2-L8) is called.

```bash
git mesh add wiki/billing \
  wiki/billing.md \
  src/checkout.ts#L2-L8
git mesh why wiki/billing -m "[why]"
```

## Charge handler
> The server-side handler [handleCharge](src/charge.ts#L2-L7) validates the schema and dispatches to Stripe.

```bash
git mesh add wiki/charge-handler \
  wiki/billing.md \
  src/charge.ts#L2-L7
git mesh why wiki/charge-handler -m "[why]"
```

---

# CLI parser • wiki/cli/parser.md

> The argument parser entrypoint is [parse_args](src/parser.rs#L2-L4).

```bash
git mesh add wiki/cli/cli-parser \
  wiki/cli/parser.md \
  src/parser.rs#L2-L4
git mesh why wiki/cli/cli-parser -m "[why]"
```

---

# Edge cases • wiki/edge.md

## git-mesh ls
> The command [git_mesh_ls](src/parser.rs#L2-L4) lists meshes touching an anchor.

```bash
git mesh add wiki/git-mesh-ls \
  wiki/edge.md \
  src/parser.rs#L2-L4
git mesh why wiki/git-mesh-ls -m "[why]"
```

## Identifier predicate
> `build_index` is the entry point used by [build_index](src/index.rs#L10-L20).

```bash
git mesh add wiki/identifier-predicate \
  wiki/edge.md \
  src/index.rs#L10-L20
git mesh why wiki/identifier-predicate -m "[why]"
```

## Bold label only
> [where_anchor](src/index.rs#L25-L40)

```bash
git mesh add wiki/bold-label-only \
  wiki/edge.md \
  src/index.rs#L25-L40
git mesh why wiki/bold-label-only -m "[why]"
```

## Table opening
> After the table the [table_anchor](src/checkout.ts#L2-L8) is referenced once more.

```bash
git mesh add wiki/table-opening \
  wiki/edge.md \
  src/index.rs#L45-L60
git mesh why wiki/table-opening -m "[why]"
```

## Table opening
> After the table the [table_anchor](src/checkout.ts#L2-L8) is referenced once more.

```bash
git mesh add wiki/table-opening-2 \
  wiki/edge.md \
  src/checkout.ts#L2-L8
git mesh why wiki/table-opening-2 -m "[why]"
```

## Ordered list opening
> 1. Validates the schema before [validate_step](src/charge.ts#L2-L7) is invoked.

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

---

# Incremental indexing • wiki/perf/indexing.md

> See [bootstrap](src/index.rs#L1-L5) for the entry point.

```bash
git mesh add wiki/perf/bootstrap \
  wiki/perf/indexing.md \
  src/index.rs#L1-L5
git mesh why wiki/perf/bootstrap -m "[why]"
```

## Sync detection
> The WikiIndex sync detects changes incrementally. It probes git state
> and computes a diff against the last snapshot at [build_index](src/index.rs#L10-L20)
> and again at [build_index](src/index.rs#L10-L20).

```bash
git mesh add wiki/perf/sync-detection \
  wiki/perf/indexing.md \
  src/index.rs#L10-L20
git mesh why wiki/perf/sync-detection -m "[why]"
```

## Apply phase
> The indexer applies each diff entry to the in-memory tree; the implementation
> spans [apply_changes](src/index.rs#L25-L40) and [apply_changes_batch](src/index.rs#L45-L60).

```bash
git mesh add wiki/perf/apply-phase \
  wiki/perf/indexing.md \
  src/index.rs#L25-L40
git mesh why wiki/perf/apply-phase -m "[why]"
```

## Apply phase
> The indexer applies each diff entry to the in-memory tree; the implementation
> spans [apply_changes](src/index.rs#L25-L40) and [apply_changes_batch](src/index.rs#L45-L60).

```bash
git mesh add wiki/perf/apply-phase-2 \
  wiki/perf/indexing.md \
  src/index.rs#L45-L60
git mesh why wiki/perf/apply-phase-2 -m "[why]"
```

## Cache layer
> The cache wiki section describes the LRU cache used by index lookups,
> backed by [CacheKey](src/index.rs#L70-L80).

```bash
git mesh add wiki/perf/cache-layer \
  wiki/perf/indexing.md \
  src/index.rs#L70-L80
git mesh why wiki/perf/cache-layer -m "[why]"
```

