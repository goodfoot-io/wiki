#!/usr/bin/env node
/**
 * Migrate bare-path fragment links to use `/` prefix where the target exists
 * at the repo-relative location.
 *
 * Bare-path links like `[text](images/photo.png)` originally resolved
 * repo-relative but now resolve page-relative (standard markdown). This
 * script prepends `/` to links whose target exists at the repo-relative
 * location, ensuring they continue to work after the resolution change.
 *
 * Usage:
 *   node scripts/migrate-links.mjs                       # migrate all
 *   node scripts/migrate-links.mjs --dry-run             # preview only
 *   node scripts/migrate-links.mjs --repo-root /path     # custom root
 */

import { readFileSync, writeFileSync, readdirSync, statSync, existsSync } from "fs";
import { join, resolve, dirname } from "path";

const URL_SCHEME_RE = /^[a-zA-Z][a-zA-Z0-9+\-.]*:/;

// ── CLI args ──────────────────────────────────────────────────────────────────

const args = process.argv.slice(2);
const dryRun = args.includes("--dry-run");
const repoRoot = resolve(argValue(args, "--repo-root") ?? ".");

function argValue(args, flag) {
  const i = args.indexOf(flag);
  return i !== -1 && i + 1 < args.length ? args[i + 1] : null;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/**
 * Check whether a bare-path link's target file exists at a given resolution.
 * Strips the fragment (#...) before checking existence.
 */
function targetExists(resolvedDir, href) {
  const hashIdx = href.indexOf("#");
  const pathPart = hashIdx === -1 ? href : href.slice(0, hashIdx);
  if (!pathPart) return false; // anchor-only
  const abs = join(resolvedDir, pathPart);
  return existsSync(abs);
}

/**
 * Decide whether a bare link should be migrated to `/`-prefixed.
 *
 * @param {string} href - The full href from the markdown link (e.g. "path/to/file.rs#L10")
 * @param {string} sourceDir - The directory of the source file containing the link
 * @returns {string|null} The new href with `/` prefix, or null to leave unchanged
 */
function migrateHref(href, sourceDir) {
  // Skip external URLs
  if (URL_SCHEME_RE.test(href)) return null;

  // Skip anchor-only links
  if (href.startsWith("#")) return null;

  // Skip links that already have a prefix
  if (href.startsWith("/") || href.startsWith("./") || href.startsWith("../")) return null;

  // Skip wikilinks ([[...]]) — these are delimited differently but just in case
  // a wikilink was somehow extracted as a bare href, skip anything with [[ or ]]
  if (href.includes("[[")) return null;

  // This is a bare path. Check existence.
  const pageRelativeExists = targetExists(sourceDir, href);
  const repoRelativeExists = targetExists(repoRoot, href);

  if (pageRelativeExists && !repoRelativeExists) {
    // Only exists page-relative — keep bare (it was page-relative intent)
    return null;
  }

  if (repoRelativeExists) {
    // Exists repo-relative — prepend `/`
    return "/" + href;
  }

  // Neither exists — default to `/` prefix
  return "/" + href;
}

/**
 * Rewrite a single line of markdown, processing all [text](href) links.
 * Inline code spans are blanked before matching to avoid false positives.
 */
function rewriteLine(line, sourceDir) {
  // Blank inline code spans so we don't accidentally match links inside them.
  const scrubbed = line.replace(/`[^`\n]*`/g, (m) => " ".repeat(m.length));

  const linkRe = /\[([^\[\]]*)\]\(([^)]*)\)/g;
  let out = "";
  let lastIndex = 0;
  let match;

  while ((match = linkRe.exec(scrubbed)) !== null) {
    const [full, , href] = match;
    const start = match.index;
    out += line.slice(lastIndex, start);

    const newHref = migrateHref(href, sourceDir);
    if (newHref !== null) {
      // Reconstruct using original text for the label part
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

/**
 * Rewrite all markdown links in content, skipping fenced code blocks and
 * inline code so we don't corrupt example text.
 */
function rewriteContent(content, sourceFile) {
  const sourceDir = dirname(sourceFile);
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

    // Outside fenced blocks: rewrite links
    result.push(rewriteLine(line, sourceDir));
  }

  return result.join("\n");
}

// ── File discovery ────────────────────────────────────────────────────────────

/**
 * Directories to exclude from recursive walks (same as `ignore` crate's
 * standard filters: build outputs, dependency dirs, hidden dirs).
 */
const EXCLUDE_DIRS = new Set(["node_modules", ".git", ".claude", "plugins", "npm", "target"]);

/**
 * Find every `wiki.toml` under `dir` and return its parent directory
 * (the wiki root).  Mirrors `find_descendant_tomls` in the CLI.
 */
function discoverWikiRoots(dir) {
  const roots = [];
  function walk(dir) {
    let entries;
    try { entries = readdirSync(dir); } catch { return; }
    for (const entry of entries) {
      if (entry.startsWith(".") || EXCLUDE_DIRS.has(entry)) continue;
      const full = join(dir, entry);
      let st;
      try { st = statSync(full); } catch { continue; }
      if (st.isDirectory()) {
        walk(full);
      } else if (entry === "wiki.toml") {
        roots.push(dir);
      }
    }
  }
  walk(dir);
  return roots;
}

/**
 * Recursively collect all `.md` files under `dir`, excluding standard
 * non-source directories.
 */
function collectMdFiles(dir, files = []) {
  let entries;
  try { entries = readdirSync(dir); } catch { return files; }
  for (const entry of entries) {
    if (entry.startsWith(".") || EXCLUDE_DIRS.has(entry)) continue;
    const full = join(dir, entry);
    let st;
    try { st = statSync(full); } catch { continue; }
    if (st.isDirectory()) {
      collectMdFiles(full, files);
    } else if (entry.endsWith(".md")) {
      files.push(full);
    }
  }
  return files;
}

/**
 * Collect all `*.wiki.md` files under `dir` that are NOT inside any of
 * `wikiRoots` (those are already covered by `collectMdFiles`).
 */
function collectWikiMdFiles(dir, wikiRoots, files = []) {
  let entries;
  try { entries = readdirSync(dir); } catch { return files; }
  for (const entry of entries) {
    if (entry.startsWith(".") || EXCLUDE_DIRS.has(entry)) continue;
    const full = join(dir, entry);
    let st;
    try { st = statSync(full); } catch { continue; }
    if (st.isDirectory()) {
      // Skip directories that are wiki roots (already covered)
      if (!wikiRoots.has(full)) {
        collectWikiMdFiles(full, wikiRoots, files);
      }
    } else if (entry.endsWith(".wiki.md")) {
      files.push(full);
    }
  }
  return files;
}

/**
 * Discover all wiki document files in the repo, matching the same path
 * discovery rules as the `wiki` CLI:
 *
 * 1. Every `.md` file under directories that contain a `wiki.toml`.
 * 2. Every `*.wiki.md` file anywhere in the repo (not already covered).
 */
function collectWikiFiles(repoRoot) {
  const absRoot = resolve(repoRoot);
  const wikiRootPaths = discoverWikiRoots(absRoot);
  const wikiRootSet = new Set(wikiRootPaths);
  const files = [];

  for (const root of wikiRootPaths) {
    collectMdFiles(root, files);
  }

  collectWikiMdFiles(absRoot, wikiRootSet, files);
  return files;
}

// ── Main ──────────────────────────────────────────────────────────────────────

function main() {
  const files = collectWikiFiles(repoRoot);

  let filesChanged = 0;
  let linksRewritten = 0;
  let linksSkipped = 0;

  for (const file of files) {
    const original = readFileSync(file, "utf8");
    const rewritten = rewriteContent(original, file);

    if (rewritten !== original) {
      const relPath = file.startsWith(repoRoot + "/") ? file.slice(repoRoot.length + 1) : file;
      const diff = countRewrittenLinks(original, rewritten);

      if (dryRun) {
        console.log(`[DRY RUN] would update: ${relPath} (${diff} link(s))`);
      } else {
        writeFileSync(file, rewritten, "utf8");
        console.log(`updated: ${relPath} (${diff} link(s))`);
      }

      filesChanged++;
      linksRewritten += diff;
    } else {
      linksSkipped += countBareLinks(original);
    }
  }

  console.log(`\n${dryRun ? "[DRY RUN] " : ""}Done.`);
  console.log(`  Files changed:   ${filesChanged}`);
  console.log(`  Links rewritten: ${linksRewritten}`);
  console.log(`  Links skipped:   ${linksSkipped} (already correct or page-relative intent)`);
}

/**
 * Count how many links were actually rewritten between original and rewritten content.
 */
function countRewrittenLinks(original, rewritten) {
  const origLines = original.split("\n");
  const rewrittenLines = rewritten.split("\n");
  let count = 0;

  const linkRe = /\[([^\[\]]*)\]\(([^)]*)\)/g;
  for (let i = 0; i < origLines.length; i++) {
    if (origLines[i] !== rewrittenLines[i]) {
      // Count the links in the original line
      const scrubbed = origLines[i].replace(/`[^`\n]*`/g, (m) => " ".repeat(m.length));
      let m;
      while ((m = linkRe.exec(scrubbed)) !== null) {
        count++;
      }
    }
  }
  return count;
}

/**
 * Count bare-path links in a file content (for skipped reporting).
 */
function countBareLinks(content) {
  const scrubbed = content.replace(/```[\s\S]*?```/g, (m) => " ".repeat(m.length))
    .replace(/`[^`\n]*`/g, (m) => " ".repeat(m.length));

  const linkRe = /\[([^\[\]]*)\]\(([^)]*)\)/g;
  let count = 0;
  let m;
  while ((m = linkRe.exec(scrubbed)) !== null) {
    const href = m[2];
    if (href.startsWith("/") || href.startsWith("./") || href.startsWith("../")) continue;
    if (href.startsWith("#")) continue;
    if (URL_SCHEME_RE.test(href)) continue;
    count++;
  }
  return count;
}

main();
