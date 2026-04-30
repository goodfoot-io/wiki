---
title: Edge cases
summary: Pages that exercise the scaffold's degenerate-excerpt and heading-shape paths.
---

# Edge cases

## `git-mesh ls`

The command [git_mesh_ls](src/parser.rs#L2-L4) lists meshes touching an anchor.

## Identifier predicate

`build_index` is the entry point used by [build_index](src/index.rs#L10-L20).

## Bold label only

**Where:**

[where_anchor](src/index.rs#L25-L40)

## Table opening

| Column | Value |
|---|---|
| anchor | [table_anchor](src/index.rs#L45-L60) |

After the table the [table_anchor](src/checkout.ts#L2-L8) is referenced once more.

## Ordered list opening

1. Validates the schema before [validate_step](src/charge.ts#L2-L7) is invoked.

## Truly degenerate

1.

[x](src/index.rs#L70-L80)
