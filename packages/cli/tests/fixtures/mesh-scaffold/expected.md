# Charge handler notes • src/notes.md

```bash
git mesh add wiki/src/charge-handler-notes \
  src/notes.md#L6-L8 \
  src/charge.ts#L2-L7
```

---

# Billing • wiki/billing.md

```bash
git mesh add wiki/billing \
  wiki/billing.md#L6-L8 \
  src/checkout.ts#L2-L8
```

## Charge handler

```bash
git mesh add wiki/charge-handler \
  wiki/billing.md#L10-L14 \
  src/charge.ts#L2-L7
```

---

# CLI parser • wiki/cli/parser.md

```bash
git mesh add wiki/cli/cli-parser \
  wiki/cli/parser.md#L6-L10 \
  src/parser.rs#L2-L4
```

---

# Edge cases • wiki/edge.md

## git-mesh ls

```bash
git mesh add wiki/git-mesh-ls \
  wiki/edge.md#L8-L10 \
  src/parser.rs#L2-L4
```

## Identifier predicate

```bash
git mesh add wiki/identifier-predicate \
  wiki/edge.md#L12-L14 \
  src/index.rs#L10-L20
```

## Bold label only

```bash
git mesh add wiki/bold-label-only \
  wiki/edge.md#L16-L20 \
  src/index.rs#L25-L40
```

## Table opening

```bash
git mesh add wiki/table-opening \
  wiki/edge.md#L22-L28 \
  src/index.rs#L45-L60 \
  src/checkout.ts#L2-L8
```

## Ordered list opening

```bash
git mesh add wiki/ordered-list-opening \
  wiki/edge.md#L30-L32 \
  src/charge.ts#L2-L7
```

## Truly degenerate

```bash
git mesh add wiki/truly-degenerate \
  wiki/edge.md#L34-L38 \
  src/index.rs#L70-L80
```

---

# Incremental indexing • wiki/perf/indexing.md

```bash
git mesh add wiki/perf/bootstrap \
  wiki/perf/indexing.md#L6-L6 \
  src/index.rs#L1-L5
```

## Sync detection

```bash
git mesh add wiki/perf/sync-detection \
  wiki/perf/indexing.md#L10-L14 \
  src/index.rs#L10-L20
```

## Apply phase

```bash
git mesh add wiki/perf/apply-phase \
  wiki/perf/indexing.md#L16-L19 \
  src/index.rs#L25-L40 \
  src/index.rs#L45-L60
```

## Cache layer

```bash
git mesh add wiki/perf/cache-layer \
  wiki/perf/indexing.md#L21-L24 \
  src/index.rs#L70-L80
```

