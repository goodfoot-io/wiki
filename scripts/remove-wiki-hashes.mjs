#!/usr/bin/env node
// Removes pinned commit SHAs from wiki fragment links.
//
// Before: [text](path#L10-L20&abc1234)  →  After: [text](path#L10-L20)
// Before: [text](path#abc1234)           →  After: [text](path)
//
// Operates on $WIKI_DIR/**/*.md (default: ./wiki) and **/*.wiki.md files.

import { readFileSync, writeFileSync, readdirSync, statSync } from "fs";
import { join, resolve } from "path";

const SHA_RE = /^[0-9a-fA-F]{7,40}$/;

// Rewrite a single href, stripping the SHA while preserving the line range.
function rewriteHref(href) {
  const hashIdx = href.indexOf("#");
  if (hashIdx === -1) return href;

  const path = href.slice(0, hashIdx);
  const fragment = href.slice(hashIdx + 1);

  const ampIdx = fragment.indexOf("&");
  if (ampIdx !== -1) {
    // Format: #L10-L20&sha  →  keep line range, drop &sha
    const lineRange = fragment.slice(0, ampIdx);
    const maybesha = fragment.slice(ampIdx + 1);
    if (SHA_RE.test(maybesha)) {
      return lineRange ? `${path}#${lineRange}` : path;
    }
    return href; // not a SHA, leave untouched
  }

  // Format: #sha  →  drop fragment entirely
  if (SHA_RE.test(fragment)) {
    return path;
  }

  return href; // plain line range or heading, leave untouched
}

// Rewrite all markdown links in content, skipping fenced code blocks and
// inline code so we don't corrupt example text.
function rewriteContent(content) {
  const lines = content.split("\n");
  const result = [];
  let inFence = false;
  let fenceChar = "";

  for (const line of lines) {
    const trimmed = line.trimStart();

    // Track fenced code blocks (``` or ~~~)
    if (!inFence) {
      if (trimmed.startsWith("```") || trimmed.startsWith("~~~")) {
        inFence = true;
        fenceChar = trimmed[0];
        result.push(line);
        continue;
      }
    } else {
      if (trimmed.startsWith(fenceChar.repeat(3))) {
        inFence = false;
      }
      result.push(line);
      continue;
    }

    // Outside fenced blocks: rewrite [text](href) links, skipping inline code
    // We process the line character by character to avoid matching inside `code`.
    result.push(rewriteLine(line));
  }

  return result.join("\n");
}

function rewriteLine(line) {
  // Blank inline code spans so we don't accidentally match links inside them.
  const scrubbed = line.replace(/`[^`\n]*`/g, (m) => " ".repeat(m.length));

  // Find all [text](href) patterns in the scrubbed line, then apply rewrites
  // back to the original line by offset.
  const linkRe = /\[([^\[\]]*)\]\(([^)]*)\)/g;
  let out = "";
  let lastIndex = 0;
  let match;

  while ((match = linkRe.exec(scrubbed)) !== null) {
    const [full, , href] = match;
    const start = match.index;
    out += line.slice(lastIndex, start);

    const newHref = rewriteHref(href);
    if (newHref !== href) {
      // Reconstruct using original text (not scrubbed) for the label part
      const originalFull = line.slice(start, start + full.length);
      // Replace only the href portion within the original match
      const hrefStart = originalFull.indexOf("](") + 2;
      out += originalFull.slice(0, hrefStart) + newHref + ")";
    } else {
      out += line.slice(start, start + full.length);
    }

    lastIndex = start + full.length;
  }

  out += line.slice(lastIndex);
  return out;
}

// Recursively collect .md files under a directory.
function collectMd(dir, files = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name.startsWith(".")) continue;
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      collectMd(full, files);
    } else if (entry.isFile() && entry.name.endsWith(".md")) {
      files.push(full);
    }
  }
  return files;
}

// Collect **/*.wiki.md files under root, excluding the wiki dir itself.
function collectWikiMd(root, wikiDir, files = []) {
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    if (entry.name.startsWith(".")) continue;
    const full = join(root, entry.name);
    if (entry.isDirectory()) {
      if (full === wikiDir) continue; // already handled separately
      collectWikiMd(full, wikiDir, files);
    } else if (entry.isFile() && entry.name.endsWith(".wiki.md")) {
      files.push(full);
    }
  }
  return files;
}

function main() {
  const repoRoot = resolve(".");
  const wikiDirName = process.env.WIKI_DIR ?? "wiki";
  const wikiDir = resolve(wikiDirName);

  const files = [];

  try {
    statSync(wikiDir);
    collectMd(wikiDir, files);
  } catch {
    // WIKI_DIR doesn't exist — skip it
  }

  collectWikiMd(repoRoot, wikiDir, files);

  let changed = 0;
  for (const file of files) {
    const original = readFileSync(file, "utf8");
    const rewritten = rewriteContent(original);
    if (rewritten !== original) {
      writeFileSync(file, rewritten, "utf8");
      console.log(`updated: ${file.replace(repoRoot + "/", "")}`);
      changed++;
    }
  }

  console.log(`\n${changed} file(s) updated.`);
}

main();
