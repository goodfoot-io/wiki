export function segments(text) {
  const s = String(text).replace(/\\\r?\n/g, " ");
  const out = [];
  let cur = "";
  let q = null;
  let i = 0;

  const flush = () => {
    const t = cur.trim();
    if (t) out.push(t);
    cur = "";
  };

  while (i < s.length) {
    const ch = s[i];

    if (q === "'") {
      cur += ch;
      if (ch === "'") q = null;
      i++;
      continue;
    }

    const quoted = q === '"';
    if (quoted && ch === "\\") {
      cur += ch + (s[i + 1] ?? "");
      i += 2;
      continue;
    }
    if (quoted && ch === '"') {
      q = null;
      cur += ch;
      i++;
      continue;
    }

    if (ch === "$" && s[i + 1] === "(") {
      flush();
      let depth = 1;
      let j = i + 2;
      let inner = "";
      while (j < s.length && depth > 0) {
        if (s[j] === "(") depth++;
        else if (s[j] === ")") {
          depth--;
          if (depth === 0) break;
        }
        inner += s[j];
        j++;
      }
      out.push(...segments(inner));
      cur = "";
      i = j + 1;
      if (quoted) q = '"';
      continue;
    }

    if (ch === "`") {
      flush();
      const end = s.indexOf("`", i + 1);
      const inner = end === -1 ? s.slice(i + 1) : s.slice(i + 1, end);
      out.push(...segments(inner));
      cur = "";
      i = end === -1 ? s.length : end + 1;
      if (quoted) q = '"';
      continue;
    }

    if (!quoted) {
      if (ch === "'") {
        q = "'";
        cur += ch;
        i++;
        continue;
      }
      if (ch === '"') {
        q = '"';
        cur += ch;
        i++;
        continue;
      }
      if ((ch === "&" && s[i + 1] === "&") || (ch === "|" && s[i + 1] === "|")) {
        flush();
        i += 2;
        continue;
      }
      if (ch === ";" || ch === "\n" || ch === "|") {
        flush();
        i++;
        continue;
      }
    }

    cur += ch;
    i++;
  }
  flush();
  return out;
}

export function splitArgs(s) {
  const out = [];
  let cur = "";
  let sq = null;
  for (const ch of String(s)) {
    if (sq) {
      if (ch === sq) sq = null;
      else cur += ch;
    } else if (ch === '"' || ch === "'") {
      sq = ch;
    } else if ("(){}[]".includes(ch)) {
      continue;
    } else if (/\s/.test(ch)) {
      if (cur) {
        out.push(cur);
        cur = "";
      }
    } else {
      cur += ch;
    }
  }
  if (cur) out.push(cur);
  return out;
}

const PREFIX_WORDS = new Set([
  "time",
  "timeout",
  "sudo",
  "exec",
  "nohup",
  "env",
  "xargs",
  "nice",
  "stdbuf",
  "watch",
  "cd",
  "command",
  "builtin",
  "then",
  "do",
  "else",
  "elif",
  "fi",
  "done",
]);

const CONSUME_ONE = new Set(["cd", "timeout"]);

export function commandToken(segment) {
  const tokens = splitArgs(segment);
  let i = 0;
  while (i < tokens.length) {
    const t = tokens[i];
    if (/^[A-Za-z_][A-Za-z0-9_]*=/.test(t)) {
      i++;
      continue;
    }
    if (PREFIX_WORDS.has(t)) {
      i++;
      if (CONSUME_ONE.has(t) && i < tokens.length && !PREFIX_WORDS.has(tokens[i])) i++;
      continue;
    }
    return { bin: t, argv: tokens.slice(i + 1) };
  }
  return null;
}

export function isWikiBinary(token) {
  if (typeof token !== "string") return false;
  const base = token.split("/").pop();
  return base === "wiki" || base === "wiki.exe";
}

export function inferSub(argv) {
  const first = argv.find((a) => !a.startsWith("-"));
  if (!first) return null;
  return /^[a-z][a-z0-9:_-]*$/i.test(first) ? first : null;
}

export function extractInvocations(text) {
  const out = [];
  for (const seg of segments(text)) {
    const parsed = commandToken(seg);
    if (!parsed || !isWikiBinary(parsed.bin)) continue;
    const argv = parsed.argv;
    const sub = inferSub(argv);
    const flags = argv.filter((a) => a.startsWith("-"));
    const positional = argv.filter((a) => !a.startsWith("-"));
    const query =
      sub === null && positional.length > 0 && /\s/.test(positional[0]) ? positional[0] : null;
    out.push({ segment: seg, bin: parsed.bin, argv, sub, flags, positional, query });
  }
  return out;
}
