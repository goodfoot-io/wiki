# Charge handler notes • src/notes.wiki.md

```bash
git mesh add src/charge-handler-notes \
  src/notes.wiki.md#L6-L8 \
  src/charge.ts#L2-L7
git mesh why src/charge-handler-notes -m "[why]"
```

---

# Billing • wiki/billing.md

```bash
git mesh add wiki/billing \
  wiki/billing.md#L6-L8 \
  src/checkout.ts#L2-L8
git mesh why wiki/billing -m "[why]"
```

## Charge handler

```bash
git mesh add wiki/charge-handler \
  wiki/billing.md#L10-L14 \
  src/charge.ts#L2-L7
git mesh why wiki/charge-handler -m "[why]"
```

---

# CLI parser • wiki/cli/parser.md

```bash
git mesh add wiki/cli/cli-parser \
  wiki/cli/parser.md#L6-L10 \
  src/parser.rs#L2-L4
git mesh why wiki/cli/cli-parser -m "[why]"
```

---

# Edge cases • wiki/edge.md

## git-mesh ls

```bash
git mesh add wiki/git-mesh-ls \
  wiki/edge.md#L8-L10 \
  src/parser.rs#L2-L4
git mesh why wiki/git-mesh-ls -m "[why]"
```

## Identifier predicate

```bash
git mesh add wiki/identifier-predicate \
  wiki/edge.md#L12-L14 \
  src/index.rs#L10-L20
git mesh why wiki/identifier-predicate -m "[why]"
```

## Bold label only

```bash
git mesh add wiki/bold-label-only \
  wiki/edge.md#L16-L20 \
  src/index.rs#L25-L40
git mesh why wiki/bold-label-only -m "[why]"
```

## Table opening

```bash
git mesh add wiki/table-opening \
  wiki/edge.md#L22-L28 \
  src/index.rs#L45-L60 \
  src/checkout.ts#L2-L8
git mesh why wiki/table-opening -m "[why]"
```

## Ordered list opening

```bash
git mesh add wiki/ordered-list-opening \
  wiki/edge.md#L30-L32 \
  src/charge.ts#L2-L7
git mesh why wiki/ordered-list-opening -m "[why]"
```

## Truly degenerate

```bash
git mesh add wiki/truly-degenerate \
  wiki/edge.md#L34-L38 \
  src/index.rs#L70-L80
git mesh why wiki/truly-degenerate -m "[why]"
```

---

# Incremental indexing • wiki/perf/indexing.md

```bash
git mesh add wiki/perf/bootstrap \
  wiki/perf/indexing.md#L6-L6 \
  src/index.rs#L1-L5
git mesh why wiki/perf/bootstrap -m "[why]"
```

## Sync detection

```bash
git mesh add wiki/perf/sync-detection \
  wiki/perf/indexing.md#L10-L14 \
  src/index.rs#L10-L20
git mesh why wiki/perf/sync-detection -m "[why]"
```

## Apply phase

```bash
git mesh add wiki/perf/apply-phase \
  wiki/perf/indexing.md#L16-L19 \
  src/index.rs#L25-L40 \
  src/index.rs#L45-L60
git mesh why wiki/perf/apply-phase -m "[why]"
```

## Cache layer

```bash
git mesh add wiki/perf/cache-layer \
  wiki/perf/indexing.md#L21-L24 \
  src/index.rs#L70-L80
git mesh why wiki/perf/cache-layer -m "[why]"
```

