#!/bin/sh
# wiki mesh scaffold — 18 uncovered findings → 16 proposed meshes (consolidation ratio 1.12×)
# Pages covered: 5

# Pre-flight: anchored paths must exist in HEAD before mesh commit.
# All anchored paths exist in HEAD.

# ── src/notes.wiki.md ───────────────────────────────────────────

# Source: # Charge handler notes
#   "Implementation lives in handleCharge."
git mesh add src/charge-handler-notes \
  src/notes.wiki.md \
  src/charge.ts#L2-L7
git mesh why src/charge-handler-notes -m ""

# ── wiki/billing.md ─────────────────────────────────────────────

# Source: # Billing
#   "The billing service validates the checkout payload before submitCheckout is called."
git mesh add wiki/billing \
  wiki/billing.md \
  src/checkout.ts#L2-L8
git mesh why wiki/billing -m ""

# Source: ## Charge handler
#   "The server-side handler handleCharge validates the schema and dispatches to Stripe."
git mesh add wiki/charge-handler \
  wiki/billing.md \
  src/charge.ts#L2-L7
git mesh why wiki/charge-handler -m ""

# ── wiki/cli/parser.md ──────────────────────────────────────────

# Source: # CLI parser
#   "The argument parser entrypoint is parse_args."
# Consolidated 2 occurrences of this anchor set
git mesh add wiki/cli/cli-parser \
  wiki/cli/parser.md \
  src/parser.rs#L2-L4
git mesh why wiki/cli/cli-parser -m ""

# ── wiki/edge.md ────────────────────────────────────────────────

# Source: ## git-mesh ls
#   "The command git_mesh_ls lists meshes touching an anchor."
git mesh add wiki/git-mesh-ls \
  wiki/edge.md \
  src/parser.rs#L2-L4
git mesh why wiki/git-mesh-ls -m ""

# Source: ## Identifier predicate
#   "build_index is the entry point used by build_index."
# WARN: source sentence opens with a bare identifier predicate;
#       the why should name the subsystem rather than restating the symbol.
git mesh add wiki/identifier-predicate \
  wiki/edge.md \
  src/index.rs#L10-L20
git mesh why wiki/identifier-predicate -m ""

# Source: ## Bold label only
#   "where_anchor"
git mesh add wiki/bold-label-only \
  wiki/edge.md \
  src/index.rs#L25-L40
git mesh why wiki/bold-label-only -m ""

# Source: ## Table opening
#   "After the table the table_anchor is referenced once more."
git mesh add wiki/table-opening \
  wiki/edge.md \
  src/index.rs#L45-L60
git mesh why wiki/table-opening -m ""

# Source: ## Table opening
#   "After the table the table_anchor is referenced once more."
git mesh add wiki/table-opening-2 \
  wiki/edge.md \
  src/checkout.ts#L2-L8
git mesh why wiki/table-opening-2 -m ""

# Source: ## Ordered list opening
#   "Validates the schema before validate_step is invoked."
# WARN: source sentence opens with a verb rather than a subject;
#       the why should name the subsystem and what it does across the anchors.
git mesh add wiki/ordered-list-opening \
  wiki/edge.md \
  src/charge.ts#L2-L7
git mesh why wiki/ordered-list-opening -m ""

# Source: ## Truly degenerate
#   "1."
# WARN: degenerate excerpt — open the source page to write the why by hand.
git mesh add wiki/truly-degenerate \
  wiki/edge.md \
  src/index.rs#L70-L80
git mesh why wiki/truly-degenerate -m ""

# ── wiki/perf/indexing.md ───────────────────────────────────────

# Source: (top of file)
#   "See bootstrap for the entry point."
# TODO: rename — fallback derivation (no section heading above link; used link label)
git mesh add wiki/perf/bootstrap \
  wiki/perf/indexing.md \
  src/index.rs#L1-L5
git mesh why wiki/perf/bootstrap -m ""

# Source: ## Sync detection
#   "The WikiIndex sync detects changes incrementally."
# Consolidated 2 occurrences of this anchor set
git mesh add wiki/perf/sync-detection \
  wiki/perf/indexing.md \
  src/index.rs#L10-L20
git mesh why wiki/perf/sync-detection -m ""

# Source: ## Apply phase
#   "The indexer applies each diff entry to the in-memory tree; the implementation spans apply_changes and apply_changes_batch."
# Consider merging with wiki/perf/apply-phase-2
git mesh add wiki/perf/apply-phase \
  wiki/perf/indexing.md \
  src/index.rs#L25-L40
git mesh why wiki/perf/apply-phase -m ""

# Source: ## Apply phase
#   "The indexer applies each diff entry to the in-memory tree; the implementation spans apply_changes and apply_changes_batch."
# Consider merging with wiki/perf/apply-phase
git mesh add wiki/perf/apply-phase-2 \
  wiki/perf/indexing.md \
  src/index.rs#L45-L60
git mesh why wiki/perf/apply-phase-2 -m ""

# Source: ## Cache layer
#   "The cache wiki section describes the LRU cache used by index lookups, backed by CacheKey."
# WARN: source sentence describes the coupling rather than the subsystem;
#       the why should name the subsystem and what it does across the anchors.
git mesh add wiki/perf/cache-layer \
  wiki/perf/indexing.md \
  src/index.rs#L70-L80
git mesh why wiki/perf/cache-layer -m ""

# Run after reviewing whys above:
git mesh commit src/charge-handler-notes
git mesh commit wiki/billing
git mesh commit wiki/charge-handler
git mesh commit wiki/cli/cli-parser
git mesh commit wiki/git-mesh-ls
git mesh commit wiki/identifier-predicate
git mesh commit wiki/bold-label-only
git mesh commit wiki/table-opening
git mesh commit wiki/table-opening-2
git mesh commit wiki/ordered-list-opening
git mesh commit wiki/truly-degenerate
git mesh commit wiki/perf/bootstrap
git mesh commit wiki/perf/sync-detection
git mesh commit wiki/perf/apply-phase
git mesh commit wiki/perf/apply-phase-2
git mesh commit wiki/perf/cache-layer
