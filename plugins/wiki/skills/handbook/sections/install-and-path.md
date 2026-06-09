# Install and PATH

Before debugging the corpus, confirm the binary. On shared dev hosts the failure is usually environmental, not a wiki-content problem.

## `wiki: command not found`

The CLI isn't on the current PATH. The `wiki` binary is the Rust CLI at `packages/cli`. Build it with the rustup toolchain (the default `cargo`/`rustc` on PATH may be too old):

```bash
PATH=$HOME/.cargo/bin:$PATH cargo build --release --manifest-path packages/cli/Cargo.toml
```

…then put the resulting binary on PATH (or install it). Verify: `wiki --version`.

## A flag is rejected / behaves oddly

The installed binary predates the flag. `--print-applied`, `wiki mesh`, etc. are recent. Symptom: a flag the docs describe errors as unknown, or the pre-commit hook blocks on a version skew. Fix: rebuild and reinstall the matching version, then re-run.

## The hook fires but nothing happens

Both the pre-commit hook and the PostToolUse plugin hook **fail open** — if `wiki` isn't found on the *hook subprocess's* PATH (`command -v wiki` fails, or `spawnSync` returns ENOENT), they exit 0 silently. So "the hook didn't create my mesh" usually means `wiki` is absent from the hook's environment, not that coverage logic broke. The hook's PATH is not your interactive shell's PATH — verify the binary resolves in the subprocess context, not just at your prompt.

## Two stores, don't confuse them

- `.wiki/` — the wiki CLI's mesh store (the coverage in this handbook).
- `.mesh/` — `git mesh`'s store for non-wiki source-to-source couplings.

Both are active and coexist. `wiki mesh …` operates on `.wiki/` only. The wiki CLI never shells out to the `git mesh` binary — it depends on `git-mesh-core` as an in-process library for hashing/search, nothing more.
