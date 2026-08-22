# rk64 parity fixture

`../rk64-parity.json` is the byte-parity proof that the vendored rk64
fingerprint kernel in [packages/cli/src/rk64.rs](../../src/rk64.rs) hashes
identically to the `git-mesh-core` crate it replaced. It pins all 183 stored
anchor pairs from the `.wiki/` corpus as that corpus existed at the baseline
commit, with canonical content embedded, so the parity test stands alone —
the corpus itself was deliberately destroyed by the migration, and page edits
must never break the proof.

## Regenerating

From anywhere inside a checkout of this repository:

```bash
python3 packages/cli/tests/fixtures/rk64-parity/make_fixture.py
```

Requirements: Python ≥ 3.10 (stdlib only), and a clone whose history contains
the baseline commit (it is an ancestor of every active branch). Expected
output: `183 entries (162 current, 21 historical)`.

The script reads all target content from git history at the pinned baseline,
never the working tree:

- Baseline is resolved as tag `rk64-parity-baseline`
  (`a1ef5b2cd8c60c73227d52de553c658b3d2a0943`). If the tag is absent from a
  clone or has moved, the script falls back to the bare SHA — same object,
  same bytes. The fixture header records both name and SHA.
- For each anchor, the fingerprint of the target's canonical content at the
  baseline tree either matches the stored value (`current`) or, for the 21
  stale-by-design anchors, matches first at some older commit, located by an
  oldest-first `git log --follow` walk (`historical`, with that commit's sha).

The output must be byte-identical to the committed fixture. Any diff means
either the kernel in rk64.rs drifted from these rules or this generator was
edited — investigate before committing anything; never hand-edit the fixture.

To re-pin a different kernel revision: create a new tag for the commit whose
`.wiki/` corpus should be pinned, update `BASELINE_SHA`/`BASELINE_REF`/
`GENERATED_FROM` in `make_fixture.py`, regenerate once, and commit the new
fixture together with the constant change.
