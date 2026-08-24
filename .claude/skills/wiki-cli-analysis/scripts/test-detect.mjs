import assert from "node:assert";
import { extractInvocations, segments } from "./lib/detect.mjs";

const cases = [
  {
    name: "bare query",
    cmd: 'wiki "mesh coverage"',
    want: [{ bin: "wiki", sub: null, flags: [], query: "mesh coverage" }],
  },
  {
    name: "cd && release binary with glob arg",
    cmd: "cd packages/cli && ./target/release/wiki check --fix 'wiki/**/*.md'",
    want: [{ bin: "./target/release/wiki", sub: "check", flags: ["--fix"] }],
  },
  {
    name: "grep alternation mentioning wiki is not an invocation",
    cmd: 'grep -rn "wiki check\\|wiki pin" --include="*.md" .',
    want: [],
  },
  {
    name: "git add of wiki-named package path",
    cmd: "git add npm/wiki-darwin-arm64/package.json npm/wiki-linux-x64/package.json",
    want: [],
  },
  {
    name: "ls/stat of the binary file",
    cmd: "ls -la target/release/wiki target/debug/wiki",
    want: [],
  },
  {
    name: "strings on the binary piped to grep",
    cmd: "strings packages/cli/target/release/wiki | grep rk64",
    want: [],
  },
  {
    name: "env assignment and timeout prefixes",
    cmd: "RUST_LOG=debug timeout 10 wiki check --no-exit-code",
    want: [{ bin: "wiki", sub: "check", flags: ["--no-exit-code"] }],
  },
  {
    name: "multiline command with line continuation",
    cmd: "set -e\n./target/debug/wiki check \\\n  --fix\necho done",
    want: [{ bin: "./target/debug/wiki", sub: "check", flags: ["--fix"] }],
  },
  {
    name: "command substitution",
    cmd: 'echo "$(wiki list)"',
    want: [{ bin: "wiki", sub: "list" }],
  },
  {
    name: "pipeline stage",
    cmd: "./target/debug/wiki check 2>&1 | head -30",
    want: [{ bin: "./target/debug/wiki", sub: "check" }],
  },
  {
    name: "bare invocation no args",
    cmd: "pwd && wiki",
    want: [{ bin: "wiki", sub: null, flags: [] }],
  },
  {
    name: "variable-indirect invocation not resolved (documented limitation)",
    cmd: 'WIKI_BIN="$PWD/target/release/wiki"\n"$WIKI_BIN" check',
    want: [],
  },
];

for (const c of cases) {
  const hits = extractInvocations(c.cmd);
  assert.strictEqual(hits.length, c.want.length, `${c.name}: expected ${c.want.length} hits, got ${hits.length}`);
  c.want.forEach((w, i) => {
    for (const [k, v] of Object.entries(w)) {
      if (k === "query") continue;
      assert.deepStrictEqual(hits[i][k], v, `${c.name}: field ${k}`);
    }
  });
}

assert.deepStrictEqual(segments('grep "a | b" wiki'), ['grep "a | b" wiki'], "quoted pipe preserved");

let passed = 0;
for (const c of cases) {
  extractInvocations(c.cmd);
  passed++;
}
stdoutWrite(`${passed} detector cases pass\n`);

function stdoutWrite(s) {
  process.stdout.write(s);
}
