#!/usr/bin/env node
/**
 * Prototype for `wiki mesh scaffold`.
 *
 * Scans ./wiki/**\/*.md and **\/*.wiki.md, extracts internal fragment links
 * (those with #L line ranges), and emits the git mesh add + git mesh why
 * commands needed to create drift-tracking meshes for each link.
 *
 * Does NOT run or commit anything — output is copy-pasteable shell commands.
 *
 * Usage:
 *   node scripts/mesh-scaffold.mjs
 *   node scripts/mesh-scaffold.mjs --wiki-dir ./wiki --output shell
 *   node scripts/mesh-scaffold.mjs --output json
 */

import { readFileSync, readdirSync, statSync } from 'fs';
import { join, relative, extname, basename } from 'path';

// ── CLI args ──────────────────────────────────────────────────────────────────

const args = process.argv.slice(2);
const wikiDirArg = argValue(args, '--wiki-dir') ?? './wiki';
const outputFormat = argValue(args, '--output') ?? 'shell';  // shell | json

function argValue(args, flag) {
  const i = args.indexOf(flag);
  return i !== -1 ? args[i + 1] : null;
}

// ── File discovery ────────────────────────────────────────────────────────────

const EXCLUDE_DIRS = new Set(['node_modules', '.git', '.claude', 'plugins', 'npm', 'target']);

function findWikiFiles(root) {
  const results = [];

  function walk(dir, inWikiDir) {
    let entries;
    try { entries = readdirSync(dir); } catch { return; }

    for (const entry of entries) {
      if (EXCLUDE_DIRS.has(entry)) continue;
      const full = join(dir, entry);
      let stat;
      try { stat = statSync(full); } catch { continue; }

      if (stat.isDirectory()) {
        // Track if we're inside the wiki/ directory tree
        const nowInWiki = inWikiDir || entry === 'wiki';
        walk(full, nowInWiki);
      } else if (entry.endsWith('.wiki.md')) {
        results.push(full);
      } else if (inWikiDir && entry.endsWith('.md')) {
        results.push(full);
      }
    }
  }

  walk(root, false);
  return results;
}

// ── Markdown scrubbing (mirrors the Rust parser) ──────────────────────────────
// Blanks fenced code blocks, inline code, and HTML comments so links inside
// them are not extracted. Newlines are preserved to keep line numbers correct.

function scrubNonContent(content) {
  const chars = content.split('');
  const len = chars.length;
  let i = 0;

  function blank(start, end) {
    for (let j = start; j < end; j++) {
      if (chars[j] !== '\n') chars[j] = ' ';
    }
  }

  function indexOf(str, search, from) {
    const idx = str.indexOf(search, from);
    return idx === -1 ? null : idx;
  }

  while (i < len) {
    const rest = content.slice(i);

    // HTML comments <!-- ... -->
    if (rest.startsWith('<!--')) {
      const end = indexOf(content, '-->', i + 4);
      if (end !== null) {
        blank(i, end + 3);
        i = end + 3;
      } else {
        blank(i, len);
        i = len;
      }
      continue;
    }

    // Fenced code blocks (``` or ~~~) at start of line
    if ((i === 0 || content[i - 1] === '\n') &&
        (rest.startsWith('```') || rest.startsWith('~~~'))) {
      const fenceChar = content[i];
      let fenceLen = 3;
      while (i + fenceLen < len && content[i + fenceLen] === fenceChar) fenceLen++;

      const fence = content.slice(i, i + fenceLen);
      const bodyStart = i;

      // Skip to end of opening fence line
      let j = i + fenceLen;
      while (j < len && content[j] !== '\n') j++;
      if (j < len) j++;

      let found = false;
      while (j < len) {
        if (content.slice(j).startsWith(fence)) {
          let k = j + fenceLen;
          while (k < len && content[k] === ' ') k++;
          if (k >= len || content[k] === '\n') {
            const closeEnd = k < len ? k + 1 : k;
            blank(bodyStart, closeEnd);
            i = closeEnd;
            found = true;
            break;
          }
        }
        while (j < len && content[j] !== '\n') j++;
        if (j < len) j++;
      }
      if (!found) { blank(bodyStart, len); i = len; }
      continue;
    }

    // Inline code ` or ``
    if (content[i] === '`') {
      let tickCount = 1;
      while (i + tickCount < len && content[i + tickCount] === '`') tickCount++;
      if (tickCount < 3) {
        const closing = '`'.repeat(tickCount);
        const end = indexOf(content, closing, i + tickCount);
        if (end !== null) {
          blank(i + tickCount, end);
          i = end + tickCount;
          continue;
        }
      } else {
        i += tickCount;
        continue;
      }
    }

    i++;
  }

  return chars.join('');
}

// ── Fragment link parser ──────────────────────────────────────────────────────

const MD_LINK_RE = /\[([^\[\]]*)\]\(([^)]*)\)/g;
const URL_SCHEME_RE = /^[a-zA-Z][a-zA-Z0-9+\-.]*:\/\//;
// Matches #L10, #L10-L20, #L10-20, optionally followed by &sha (legacy)
const LINE_RANGE_RE = /^L(\d+)(?:-L?(\d+))?(?:&[0-9a-f]+)?$/i;

function parseFragmentLinks(content, wikiFile) {
  const scrubbed = scrubNonContent(content);
  const links = [];

  for (const match of scrubbed.matchAll(MD_LINK_RE)) {
    const [full, text, href] = match;
    if (URL_SCHEME_RE.test(href)) continue;

    const hashIdx = href.indexOf('#');
    if (hashIdx === -1) continue;

    const path = href.slice(0, hashIdx);
    const fragment = href.slice(hashIdx + 1);
    const rangeMatch = LINE_RANGE_RE.exec(fragment);
    if (!rangeMatch) continue;

    const startLine = parseInt(rangeMatch[1], 10);
    const endLine = rangeMatch[2] ? parseInt(rangeMatch[2], 10) : startLine;

    // Recover original (unscrubbed) link text for name generation
    const textStart = match.index + 1;
    const originalText = content.slice(textStart, textStart + match[1].length);

    // 1-based source line + the original (unscrubbed) line text for why generation
    const sourceLine = scrubbed.slice(0, match.index).split('\n').length;
    const lineText = content.split('\n')[sourceLine - 1] ?? '';

    links.push({ wikiFile, path, startLine, endLine, originalText, sourceLine, lineText });
  }

  return links;
}

// ── Mesh name generation ──────────────────────────────────────────────────────

function slugify(str) {
  return str
    .toLowerCase()
    .replace(/[`*_[\]#]/g, '')         // strip markdown punctuation
    .replace(/\.[a-z]+$/i, '')         // strip file extensions
    .replace(/'s\b/g, '')              // possessives ("Git's" → "git")
    .replace(/[^a-z0-9]+/g, '-')       // non-alphanumeric → dash
    .replace(/^-+|-+$/g, '')           // trim leading/trailing dashes
    .slice(0, 40);
}

// Use file stem as fallback when text is too generic or very long (>5 words)
function labelToSlug(text, targetPath) {
  const clean = text.replace(/[`*_[\]]/g, '').trim();
  const words = clean.split(/\s+/).filter(Boolean);
  if (words.length > 5 || words.length === 0) {
    return slugify(basename(targetPath, extname(targetPath)));
  }
  return slugify(clean);
}

// Frontmatter title cache: file path → title string
const frontmatterTitleCache = new Map();

function readFrontmatterTitle(filePath) {
  if (frontmatterTitleCache.has(filePath)) return frontmatterTitleCache.get(filePath);
  try {
    const content = readFileSync(filePath, 'utf8');
    const match = content.match(/^---\s*\n(?:.*\n)*?title:\s*(.+?)\s*\n/);
    const title = match ? match[1].replace(/^['"]|['"]$/g, '').trim() : null;
    frontmatterTitleCache.set(filePath, title);
    return title;
  } catch {
    frontmatterTitleCache.set(filePath, null);
    return null;
  }
}

function wikiTitleSlug(wikiFile) {
  // Prefer frontmatter title; fall back to filename stem
  const title = readFrontmatterTitle(wikiFile);
  if (title) return slugify(title);
  return slugify(basename(wikiFile, '.md').replace(/\.wiki$/, ''));
}

function targetSlug(link) {
  const text = link.originalText.trim();
  return text && !text.includes('/')
    ? labelToSlug(text, link.path)
    : slugify(basename(link.path, extname(link.path)));
}

function meshName(link) {
  return `wiki/${wikiTitleSlug(link.wikiFile)}/${targetSlug(link)}`;
}

function meshWhy(link) {
  const raw = link.lineText.trimStart();

  // Headings name sections, not subsystems — fall back to Documentation.
  if (raw.startsWith('#')) {
    return 'Documentation.';
  }

  // Table rows: extract prose from the non-link cells rather than falling back
  // to the label (which is typically a bare filename in the last column).
  if (raw.startsWith('|')) {
    const cells = raw
      .split('|')
      .map(c => c
        .replace(/`[^`\n]+`/g, '')
        .replace(/\[([^\[\]]*)\]\([^)]*\)/g, '$1')
        .replace(/\*\*([^*]+)\*\*/g, '$1')
        .replace(/\*([^*]+)\*/g, '$1')
        .trim()
      )
      .filter(c => c.length > 0);
    // Find the longest cell that isn't just a filename/path — that's the description
    const description = cells
      .filter(c => !/^[A-Za-z0-9_\-/.]+\.(ts|js|rs|md|json|toml|sh|mjs)$/.test(c))
      .sort((a, b) => b.length - a.length)[0];
    if (description && description.length >= 8) {
      let why = description.replace(/[.,;: ]+$/, '') + '.';
      if (why.length > 160) why = why.slice(0, 160).replace(/\s\S*$/, '') + '.';
      return why;
    }
    return 'Documentation.';
  }

  // Derive the why from the prose sentence containing the link. The wiki page
  // already describes what the code does — we just clean it up into a
  // definition: a noun phrase naming the subsystem + what it does.
  let prose = link.lineText
    // Strip inline code spans first so example syntax inside backticks isn't processed
    .replace(/`[^`\n]+`/g, '')
    // Strip markdown links [text](href) → text
    .replace(/\[([^\[\]]*)\]\([^)]*\)/g, '$1')
    // Strip wikilinks [[Title|display]] → display or Title
    .replace(/\[\[([^\]|]+)(?:\|([^\]]*))?\]\]/g, (_, title, display) => display ?? title)
    // Strip leading list/heading markers and bold/italic
    .replace(/^[#\-*0-9. ]+/, '')
    .replace(/\*\*([^*]+)\*\*/g, '$1')
    .replace(/\*([^*]+)\*/g, '$1')
    .replace(/`([^`]+)`/g, '$1')
    .trim();

  // Clean up residue from stripped spans
  prose = prose
    .replace(/\([^)]*\)/g, '')         // remove all parens + contents (empty or punctuation-only args)
    .replace(/\*+/g, '')               // leftover bold/italic markers
    .replace(/,(\s*,)+/g, ',')         // collapse repeated commas from stripped code spans
    .replace(/\s+/g, ' ')              // collapse whitespace
    .replace(/\s+([.,;:])/g, '$1')     // remove space before punctuation
    .replace(/^[,;: ]+/, '')           // strip leading punctuation (headless predicates)
    .replace(/\b(for|to|in|at|of|by|from|with)\s*\.$/, '.')  // strip dangling preposition before period
    .trim();

  // If the prose starts with a verb (headless predicate — subject was a stripped code span),
  // prepend the link label as the grammatical subject.
  const startsWithVerb = /^(is|are|was|were|applies|reserves|decides|builds|exports|stores|handles|manages|validates|parses|wraps|maps|tracks|owns|uses|caches|returns|emits|reads|writes|checks|runs|renders|sends|receives|creates|updates|deletes|fetches|loads|saves|generates|computes|resolves|detects|scans|walks|merges|splits|groups|filters|sorts|formats|logs|reports)\b/i;
  if (startsWithVerb.test(prose)) {
    const label = link.originalText.replace(/[`*_[\]]/g, '').trim();
    if (label) prose = label.charAt(0).toUpperCase() + label.slice(1) + ' ' + prose.charAt(0).toLowerCase() + prose.slice(1);
  }

  // Truncate to first sentence boundary (. ! ?), then hard-cap at 160 chars
  const sentenceEnd = prose.search(/[.!?]/);
  if (sentenceEnd !== -1) {
    prose = prose.slice(0, sentenceEnd + 1);
  } else if (prose.length > 160) {
    // No sentence boundary — cut at last word before 160 and close
    prose = prose.slice(0, 160).replace(/\s\S*$/, '') + '.';
  } else if (!prose.endsWith('.')) {
    prose += '.';
  }

  // Reject degenerate results: too short, only punctuation, or just a bare path/filename
  const stripped = prose.replace(/[.,;: ]/g, '');
  const trimmed = prose.trim();
  const looksLikePath = /^[A-Za-z0-9_\-/.]+\.(ts|js|rs|md|json|toml|sh|mjs|cjs)\.?$/i.test(trimmed)
    || /^[A-Za-z0-9_-]+\/[A-Za-z0-9_./-]+\.?$/.test(trimmed);
  const meaningful = stripped.length >= 8 && !looksLikePath;
  return meaningful ? prose : 'Documentation.';
}

// ── Deduplication ─────────────────────────────────────────────────────────────
// If two links produce the same mesh name, disambiguate with a counter.

function deduplicateNames(meshes) {
  const seen = new Map();
  for (const m of meshes) {
    const base = m.name;
    const count = (seen.get(base) ?? 0) + 1;
    seen.set(base, count);
    if (count > 1) m.name = `${base}-${count}`;
  }
  // Second pass: also suffix the first occurrence if there were collisions
  const counts = new Map();
  for (const m of meshes) {
    const base = m.name.replace(/-\d+$/, '');
    counts.set(base, (counts.get(base) ?? 0) + 1);
  }
  // Re-run with stable numbering
  const seen2 = new Map();
  for (const m of meshes) {
    const base = m.name.replace(/-\d+$/, '');
    if ((counts.get(base) ?? 1) > 1) {
      const n = (seen2.get(base) ?? 0) + 1;
      seen2.set(base, n);
      m.name = `${base}-${n}`;
    }
  }
  return meshes;
}

// ── Main ──────────────────────────────────────────────────────────────────────

const root = process.cwd();
const files = findWikiFiles(root);

if (files.length === 0) {
  console.error('No wiki files found. Run from the repo root.');
  process.exit(1);
}

const allLinks = [];
for (const file of files) {
  const content = readFileSync(file, 'utf8');
  const links = parseFragmentLinks(content, file);
  allLinks.push(...links);
}

if (allLinks.length === 0) {
  console.error('No fragment links with line ranges found.');
  process.exit(0);
}

const meshes = allLinks.map(link => ({
  name: meshName(link),
  why: meshWhy(link),
  wikiFile: relative(root, link.wikiFile).replace(/\\/g, '/'),
  anchor: `${link.path}#L${link.startLine}-L${link.endLine}`,
  sourceLine: link.sourceLine,
}));

deduplicateNames(meshes);

// ── Output ────────────────────────────────────────────────────────────────────

if (outputFormat === 'json') {
  console.log(JSON.stringify(meshes, null, 2));
  process.exit(0);
}

// Group by wiki file for readable shell output
const byFile = new Map();
for (const m of meshes) {
  if (!byFile.has(m.wikiFile)) byFile.set(m.wikiFile, []);
  byFile.get(m.wikiFile).push(m);
}

console.log('#!/bin/sh');
console.log('# Generated by scripts/mesh-scaffold.mjs');
console.log('#');
console.log('# Before running:');
console.log('#   1. Review mesh names — rename to a topical slug that names the subsystem.');
console.log('#   2. Review each why — it is derived from surrounding prose and may need');
console.log('#      editing to read as a definition of what the subsystem does, not a');
console.log('#      description of where the link came from.');
console.log('#   3. Links sharing the same why likely belong on a single mesh — merge them.');
console.log('#   4. Commit each mesh: git mesh commit <name>');
console.log();

for (const [wikiFile, entries] of byFile) {
  console.log(`# ── ${wikiFile} ${'─'.repeat(Math.max(0, 60 - wikiFile.length))}`);
  for (const m of entries) {
    console.log();
    console.log(`git mesh add ${m.name} \\`);
    console.log(`  ${m.wikiFile} \\`);
    console.log(`  ${m.anchor}`);
    console.log(`git mesh why ${m.name} -m "${m.why}"`);
  }
  console.log();
}
