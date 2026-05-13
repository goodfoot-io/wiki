#!/usr/bin/env node
/**
 * replace-wikilinks.mjs
 *
 * Converts [[…]] wikilink syntax in wiki/*.md files to plain markdown links.
 *
 * Usage:
 *   node scripts/replace-wikilinks.mjs [--dry-run]
 *
 * Algorithm:
 *   1. Build title→filepath map from `wiki list --format json`.
 *   2. Walk every .md and *.wiki.md under wiki/.
 *   3. For each [[ns:Title#Heading|Display]] match:
 *      - Resolve title (strip ns: prefix) to a filesystem path.
 *      - If #Heading present, find matching heading's line range in target file.
 *      - Emit [Display](./relative/path#Lstart-Lend) or [Title](./path).
 *      - Unresolved targets → stderr warning, skip.
 *   4. Rewrite files in-place (only [[…]] spans replaced).
 *   5. --dry-run prints diffs without writing.
 */

import { execSync } from 'child_process';
import { readFileSync, writeFileSync, readdirSync, statSync } from 'fs';
import { join, relative, dirname, resolve } from 'path';

const DRY_RUN = process.argv.includes('--dry-run');
const REPO_ROOT = resolve(new URL('.', import.meta.url).pathname, '..');
const WIKI_DIR = join(REPO_ROOT, 'wiki');

// Build title → absolute path map using wiki list --format json
function buildTitleMap() {
  let raw;
  try {
    raw = execSync('wiki list --format json', { cwd: REPO_ROOT, encoding: 'utf8' });
  } catch (e) {
    process.stderr.write(`ERROR: wiki list --format json failed: ${e.message}\n`);
    process.exit(1);
  }
  const entries = JSON.parse(raw);
  const map = new Map();
  for (const entry of entries) {
    map.set(entry.title.toLowerCase(), entry.file);
    if (entry.aliases) {
      for (const alias of entry.aliases) {
        if (!map.has(alias.toLowerCase())) {
          map.set(alias.toLowerCase(), entry.file);
        }
      }
    }
  }
  return map;
}

// Walk directory recursively, yielding .md and *.wiki.md files
function* walkMd(dir) {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    const st = statSync(full);
    if (st.isDirectory()) {
      yield* walkMd(full);
    } else if (entry.endsWith('.md')) {
      yield full;
    }
  }
}

// Find line range for a heading in a file's content lines.
// Returns [startLine, endLine] (1-based, inclusive) or null.
function findHeadingRange(lines, heading) {
  // Normalize: lowercase, replace spaces with hyphens, strip non-alphanumeric except hyphens
  function slugify(text) {
    return text
      .toLowerCase()
      .replace(/[^\w\s-]/g, '')
      .replace(/\s+/g, '-')
      .replace(/-+/g, '-');
  }

  const targetSlug = slugify(heading);
  let startLine = null;

  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(/^(#{1,6})\s+(.+)$/);
    if (m) {
      const slug = slugify(m[2]);
      if (slug === targetSlug) {
        startLine = i + 1; // 1-based
        // Find end: next heading of same or higher level, or EOF
        const level = m[1].length;
        let endLine = lines.length;
        for (let j = i + 1; j < lines.length; j++) {
          const next = lines[j].match(/^(#{1,6})\s+/);
          if (next && next[1].length <= level) {
            endLine = j; // exclusive, so last line of section is j
            break;
          }
        }
        return [startLine, endLine];
      }
    }
  }
  return null;
}

// Parse a wikilink: [[ns:Title#Heading|Display]] or [[Title#Heading|Display]] etc.
// Returns { title, heading, display } or null
function parseWikilink(inner) {
  // Strip namespace prefix (ns: or default:)
  let rest = inner.trim();
  const nsMatch = rest.match(/^[a-zA-Z0-9_-]+:/);
  if (nsMatch) {
    rest = rest.slice(nsMatch[0].length);
  }

  // Split display
  let display = null;
  const pipeIdx = rest.indexOf('|');
  if (pipeIdx !== -1) {
    display = rest.slice(pipeIdx + 1).trim();
    rest = rest.slice(0, pipeIdx).trim();
  }

  // Split heading
  let heading = null;
  const hashIdx = rest.indexOf('#');
  if (hashIdx !== -1) {
    heading = rest.slice(hashIdx + 1).trim();
    rest = rest.slice(0, hashIdx).trim();
  }

  const title = rest.trim();
  return { title, heading, display };
}

function processFile(filePath, titleMap) {
  const content = readFileSync(filePath, 'utf8');
  if (!content.includes('[[')) return;

  const wikilinkRe = /\[\[([^\]]+)\]\]/g;
  let result = content;
  let offset = 0;
  let match;
  let changed = false;

  // Reset regex
  wikilinkRe.lastIndex = 0;

  // Collect all replacements first (to avoid offset issues)
  const replacements = [];
  let m;
  while ((m = wikilinkRe.exec(content)) !== null) {
    const fullMatch = m[0];
    const inner = m[1];
    const start = m.index;
    const end = start + fullMatch.length;

    const parsed = parseWikilink(inner);
    if (!parsed) {
      process.stderr.write(`WARN: could not parse wikilink ${fullMatch} in ${filePath}\n`);
      continue;
    }

    const { title, heading, display } = parsed;

    if (!title) {
      process.stderr.write(`WARN: empty title in ${fullMatch} in ${filePath}\n`);
      continue;
    }

    const targetPath = titleMap.get(title.toLowerCase());
    if (!targetPath) {
      process.stderr.write(`WARN: unresolved wikilink [[${title}]] in ${filePath}\n`);
      continue;
    }

    // Compute relative path from linking file's directory to target
    const linkingDir = dirname(filePath);
    let relPath = relative(linkingDir, targetPath);
    // Ensure it starts with ./
    if (!relPath.startsWith('.')) relPath = './' + relPath;

    let anchor = '';
    if (heading) {
      const targetContent = readFileSync(targetPath, 'utf8');
      const lines = targetContent.split('\n');
      const range = findHeadingRange(lines, heading);
      if (range) {
        anchor = `#L${range[0]}-L${range[1]}`;
      } else {
        process.stderr.write(`WARN: heading "${heading}" not found in ${targetPath} (from ${filePath})\n`);
        // Fall back to no anchor
      }
    }

    const label = display || title;
    const replacement = `[${label}](${relPath}${anchor})`;
    replacements.push({ start, end, replacement });
  }

  if (replacements.length === 0) return;

  // Apply replacements in reverse order to preserve indices
  let newContent = content;
  for (let i = replacements.length - 1; i >= 0; i--) {
    const { start, end, replacement } = replacements[i];
    newContent = newContent.slice(0, start) + replacement + newContent.slice(end);
  }

  if (newContent === content) return;

  if (DRY_RUN) {
    console.log(`\n--- ${filePath}`);
    // Simple diff: show changed lines
    const oldLines = content.split('\n');
    const newLines = newContent.split('\n');
    for (let i = 0; i < Math.max(oldLines.length, newLines.length); i++) {
      const old = oldLines[i] ?? '';
      const nw = newLines[i] ?? '';
      if (old !== nw) {
        console.log(`  - ${old}`);
        console.log(`  + ${nw}`);
      }
    }
  } else {
    writeFileSync(filePath, newContent, 'utf8');
    console.log(`Rewrote: ${filePath} (${replacements.length} wikilink(s) replaced)`);
  }
}

// Main
const titleMap = buildTitleMap();
console.log(`Title map has ${titleMap.size} entries`);

let filesProcessed = 0;
for (const filePath of walkMd(WIKI_DIR)) {
  processFile(filePath, titleMap);
  filesProcessed++;
}

console.log(`\nDone. Processed ${filesProcessed} files.`);
if (DRY_RUN) {
  console.log('(dry-run mode — no files written)');
}
