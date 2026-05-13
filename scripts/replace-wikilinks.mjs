#!/usr/bin/env node
/**
 * replace-wikilinks.mjs
 *
 * Converts [[…]] wikilink syntax to plain markdown links.
 *
 * Usage:
 *   node scripts/replace-wikilinks.mjs [--dry-run] [root]
 *
 * Discovery uses the legacy wiki rules (this script migrates *from* that world):
 *   - any *.wiki.md file, OR
 *   - any *.md file with an ancestor directory containing a wiki.toml
 *
 * The title map is built locally from candidates whose YAML frontmatter declares
 * `title` and `summary` (with optional `aliases`). All candidates are scanned
 * for [[…]] rewrites; unresolved targets emit stderr warnings and are skipped.
 */

import { readFileSync, writeFileSync, readdirSync, statSync, existsSync } from 'fs';
import { join, relative, dirname, resolve } from 'path';

const args = process.argv.slice(2);
const DRY_RUN = args.includes('--dry-run');
const ROOT = resolve(args.find((a) => !a.startsWith('--')) ?? process.cwd());

const SKIP_DIRS = new Set(['.git', 'node_modules', 'target', 'dist', 'build', '.cache']);

function* walkAll(dir) {
  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    return;
  }
  for (const entry of entries) {
    if (SKIP_DIRS.has(entry)) continue;
    const full = join(dir, entry);
    let st;
    try {
      st = statSync(full);
    } catch {
      continue;
    }
    if (st.isDirectory()) {
      yield { kind: 'dir', path: full };
      yield* walkAll(full);
    } else if (st.isFile()) {
      yield { kind: 'file', path: full };
    }
  }
}

function collectCandidates(root) {
  const wikiTomlDirs = [];
  const allMd = [];
  for (const node of walkAll(root)) {
    if (node.kind === 'dir') continue;
    const base = node.path.slice(node.path.lastIndexOf('/') + 1);
    if (base === 'wiki.toml') wikiTomlDirs.push(dirname(node.path) + '/');
    else if (node.path.endsWith('.md')) allMd.push(node.path);
  }
  // Longest-prefix wins so nested wiki.toml roots take precedence over outer ones.
  wikiTomlDirs.sort((a, b) => b.length - a.length);
  const candidates = [];
  for (const file of allMd) {
    if (file.endsWith('.wiki.md')) {
      candidates.push(file);
      continue;
    }
    if (wikiTomlDirs.some((d) => file.startsWith(d))) {
      candidates.push(file);
    }
  }
  return { candidates, wikiTomlDirs };
}

// Returns the namespace key for a file: the nearest ancestor wiki.toml dir,
// or null for standalone *.wiki.md files (each is its own namespace island).
function namespaceOf(file, wikiTomlDirs) {
  if (file.endsWith('.wiki.md')) return null;
  return wikiTomlDirs.find((d) => file.startsWith(d)) ?? null;
}

// Minimal YAML frontmatter parser for title/summary/aliases.
// Returns { title, summary, aliases } or null if no valid frontmatter block.
function parseFrontmatter(content) {
  if (!content.startsWith('---\n') && !content.startsWith('---\r\n')) return null;
  const rest = content.slice(content.indexOf('\n') + 1);
  const endIdx = rest.indexOf('\n---');
  if (endIdx === -1) return null;
  const block = rest.slice(0, endIdx);
  const out = {};
  const lines = block.split('\n');
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const m = line.match(/^([A-Za-z_][A-Za-z0-9_-]*)\s*:\s*(.*)$/);
    if (!m) continue;
    const key = m[1];
    let val = m[2].trim();
    if (key === 'aliases') {
      // Inline [a, b] form
      const inline = val.match(/^\[(.*)\]$/);
      if (inline) {
        out.aliases = inline[1]
          .split(',')
          .map((s) => s.trim().replace(/^["']|["']$/g, ''))
          .filter(Boolean);
      } else if (val === '') {
        // Block form: subsequent `  - item` lines
        const items = [];
        for (let j = i + 1; j < lines.length; j++) {
          const lm = lines[j].match(/^\s*-\s*(.+)$/);
          if (!lm) break;
          items.push(lm[1].trim().replace(/^["']|["']$/g, ''));
        }
        out.aliases = items;
      }
    } else if (key === 'title' || key === 'summary') {
      out[key] = val.replace(/^["']|["']$/g, '');
    }
  }
  return out;
}

function buildTitleMap(candidates) {
  const map = new Map();
  for (const file of candidates) {
    let content;
    try {
      content = readFileSync(file, 'utf8');
    } catch (e) {
      process.stderr.write(`WARN: cannot read ${file}: ${e.message}\n`);
      continue;
    }
    let fm;
    try {
      fm = parseFrontmatter(content);
    } catch (e) {
      process.stderr.write(`WARN: frontmatter parse error in ${file}: ${e.message}\n`);
      continue;
    }
    if (!fm || !fm.title || !fm.summary) continue;
    const add = (key) => {
      const k = key.toLowerCase();
      if (!map.has(k)) map.set(k, file);
    };
    add(fm.title);
    if (Array.isArray(fm.aliases)) fm.aliases.forEach(add);
  }
  return map;
}

function findHeadingRange(lines, heading) {
  const slugify = (text) =>
    text
      .toLowerCase()
      .replace(/[^\w\s-]/g, '')
      .replace(/\s+/g, '-')
      .replace(/-+/g, '-');
  const targetSlug = slugify(heading);
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(/^(#{1,6})\s+(.+)$/);
    if (!m) continue;
    if (slugify(m[2]) !== targetSlug) continue;
    const level = m[1].length;
    let endLine = lines.length;
    for (let j = i + 1; j < lines.length; j++) {
      const next = lines[j].match(/^(#{1,6})\s+/);
      if (next && next[1].length <= level) {
        endLine = j;
        break;
      }
    }
    return [i + 1, endLine];
  }
  return null;
}

function parseWikilink(inner) {
  let rest = inner.trim();
  const nsMatch = rest.match(/^[a-zA-Z0-9_-]+:/);
  if (nsMatch) rest = rest.slice(nsMatch[0].length);
  let display = null;
  const pipeIdx = rest.indexOf('|');
  if (pipeIdx !== -1) {
    display = rest.slice(pipeIdx + 1).trim();
    rest = rest.slice(0, pipeIdx).trim();
  }
  let heading = null;
  const hashIdx = rest.indexOf('#');
  if (hashIdx !== -1) {
    heading = rest.slice(hashIdx + 1).trim();
    rest = rest.slice(0, hashIdx).trim();
  }
  return { title: rest.trim(), heading, display };
}

function processFile(filePath, titleMap, wikiTomlDirs) {
  const content = readFileSync(filePath, 'utf8');
  if (!content.includes('[[')) return;

  const wikilinkRe = /\[\[([^\]]+)\]\]/g;
  const replacements = [];
  let m;
  while ((m = wikilinkRe.exec(content)) !== null) {
    const fullMatch = m[0];
    const start = m.index;
    const end = start + fullMatch.length;
    const parsed = parseWikilink(m[1]);
    if (!parsed || !parsed.title) {
      process.stderr.write(`WARN: could not parse ${fullMatch} in ${filePath}\n`);
      continue;
    }
    const { title, heading, display } = parsed;
    const targetPath = titleMap.get(title.toLowerCase());
    if (!targetPath) {
      process.stderr.write(`WARN: unresolved wikilink [[${title}]] in ${filePath}\n`);
      continue;
    }
    const srcNs = namespaceOf(filePath, wikiTomlDirs);
    const tgtNs = namespaceOf(targetPath, wikiTomlDirs);
    const crossNamespace = srcNs !== tgtNs;
    const targetIsWikiMd = targetPath.endsWith('.wiki.md');
    let relPath;
    if (crossNamespace || targetIsWikiMd) {
      relPath = '/' + relative(ROOT, targetPath);
    } else {
      relPath = relative(dirname(filePath), targetPath);
      if (!relPath.startsWith('.')) relPath = './' + relPath;
    }
    let anchor = '';
    if (heading) {
      const lines = readFileSync(targetPath, 'utf8').split('\n');
      const range = findHeadingRange(lines, heading);
      if (range) anchor = `#L${range[0]}-L${range[1]}`;
      else
        process.stderr.write(
          `WARN: heading "${heading}" not found in ${targetPath} (from ${filePath})\n`,
        );
    }
    const label = display || title;
    replacements.push({ start, end, replacement: `[${label}](${relPath}${anchor})` });
  }

  if (replacements.length === 0) return;

  let newContent = content;
  for (let i = replacements.length - 1; i >= 0; i--) {
    const { start, end, replacement } = replacements[i];
    newContent = newContent.slice(0, start) + replacement + newContent.slice(end);
  }
  if (newContent === content) return;

  if (DRY_RUN) {
    console.log(`\n--- ${filePath}`);
    const oldLines = content.split('\n');
    const newLines = newContent.split('\n');
    for (let i = 0; i < Math.max(oldLines.length, newLines.length); i++) {
      const o = oldLines[i] ?? '';
      const n = newLines[i] ?? '';
      if (o !== n) {
        console.log(`  - ${o}`);
        console.log(`  + ${n}`);
      }
    }
  } else {
    writeFileSync(filePath, newContent, 'utf8');
    console.log(`Rewrote: ${filePath} (${replacements.length} wikilink(s) replaced)`);
  }
}

if (!existsSync(ROOT)) {
  process.stderr.write(`ERROR: root does not exist: ${ROOT}\n`);
  process.exit(1);
}

console.log(`Root: ${ROOT}`);
const { candidates, wikiTomlDirs } = collectCandidates(ROOT);
console.log(`Discovered ${candidates.length} legacy wiki candidate file(s)`);
const titleMap = buildTitleMap(candidates);
console.log(`Title map has ${titleMap.size} entries`);

for (const filePath of candidates) {
  processFile(filePath, titleMap, wikiTomlDirs);
}

console.log(`\nDone. Processed ${candidates.length} files.`);
if (DRY_RUN) console.log('(dry-run mode — no files written)');
