#!/usr/bin/env node
/**
 * mesh-scaffold-v2.mjs — alternate scaffold using nameRelationship() for
 * smarter name + why generation.
 *
 * Usage:
 *   node scripts/mesh-scaffold-v2.mjs
 *   node scripts/mesh-scaffold-v2.mjs --output json
 *   node scripts/mesh-scaffold-v2.mjs --show-debug
 */

import { readFileSync, readdirSync, statSync } from 'fs';
import { join, relative, extname, basename } from 'path';

const args = process.argv.slice(2);
const outputFormat = args.includes('--output') ? args[args.indexOf('--output') + 1] : 'shell';
const showDebug = args.includes('--show-debug');

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
        walk(full, inWikiDir || entry === 'wiki');
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

// ── Markdown scrubbing ────────────────────────────────────────────────────────

function scrubNonContent(content) {
  const chars = content.split('');
  const len = chars.length;
  let i = 0;

  function blank(start, end) {
    for (let j = start; j < end; j++) if (chars[j] !== '\n') chars[j] = ' ';
  }

  while (i < len) {
    const rest = content.slice(i);
    if (rest.startsWith('<!--')) {
      const end = content.indexOf('-->', i + 4);
      if (end !== -1) { blank(i, end + 3); i = end + 3; }
      else { blank(i, len); i = len; }
      continue;
    }
    if ((i === 0 || content[i - 1] === '\n') && (rest.startsWith('```') || rest.startsWith('~~~'))) {
      const fc = content[i];
      let fl = 3;
      while (i + fl < len && content[i + fl] === fc) fl++;
      const fence = content.slice(i, i + fl);
      const bodyStart = i;
      let j = i + fl;
      while (j < len && content[j] !== '\n') j++;
      if (j < len) j++;
      let found = false;
      while (j < len) {
        if (content.slice(j).startsWith(fence)) {
          let k = j + fl;
          while (k < len && content[k] === ' ') k++;
          if (k >= len || content[k] === '\n') {
            blank(bodyStart, k < len ? k + 1 : k);
            i = k < len ? k + 1 : k;
            found = true; break;
          }
        }
        while (j < len && content[j] !== '\n') j++;
        if (j < len) j++;
      }
      if (!found) { blank(bodyStart, len); i = len; }
      continue;
    }
    if (content[i] === '`') {
      let tc = 1;
      while (i + tc < len && content[i + tc] === '`') tc++;
      if (tc < 3) {
        const end = content.indexOf('`'.repeat(tc), i + tc);
        if (end !== -1) { blank(i + tc, end); i = end + tc; continue; }
      } else { i += tc; continue; }
    }
    i++;
  }
  return chars.join('');
}

// ── Fragment link + heading-chain parser ──────────────────────────────────────

const MD_LINK_RE = /\[([^\[\]]*)\]\(([^)]*)\)/g;
const URL_SCHEME_RE = /^[a-zA-Z][a-zA-Z0-9+\-.]*:\/\//;
const LINE_RANGE_RE = /^L(\d+)(?:-L?(\d+))?(?:&[0-9a-f]+)?$/i;
const HEADING_RE = /^(#{1,6})\s+(.+)/;

function parseFragmentLinks(content, wikiFile) {
  const scrubbed = scrubNonContent(content);
  const lines = content.split('\n');
  const links = [];

  // Pre-build heading chain for each line number
  const headingStack = []; // { level, text }
  const headingChainByLine = [];
  for (const line of lines) {
    const hm = HEADING_RE.exec(line.trimStart());
    if (hm) {
      const level = hm[1].length;
      const text = hm[2].replace(/[`*_[\]]/g, '').trim();
      while (headingStack.length && headingStack[headingStack.length - 1].level >= level) {
        headingStack.pop();
      }
      headingStack.push({ level, text });
    }
    headingChainByLine.push(headingStack.map(h => h.text));
  }

  for (const match of scrubbed.matchAll(MD_LINK_RE)) {
    const [, text, href] = match;
    if (URL_SCHEME_RE.test(href)) continue;
    const hashIdx = href.indexOf('#');
    if (hashIdx === -1) continue;
    const path = href.slice(0, hashIdx);
    const fragment = href.slice(hashIdx + 1);
    const rangeMatch = LINE_RANGE_RE.exec(fragment);
    if (!rangeMatch) continue;

    const startLine = parseInt(rangeMatch[1], 10);
    const endLine = rangeMatch[2] ? parseInt(rangeMatch[2], 10) : startLine;
    const textStart = match.index + 1;
    const originalText = content.slice(textStart, textStart + match[1].length);
    const sourceLine = scrubbed.slice(0, match.index).split('\n').length;
    const lineText = lines[sourceLine - 1] ?? '';
    const headingChain = headingChainByLine[sourceLine - 1] ?? [];

    // Surrounding text: up to 2 lines before and after, joined
    const surroundingLines = lines.slice(Math.max(0, sourceLine - 3), sourceLine + 2);
    const surroundingText = surroundingLines
      .join(' ')
      .replace(/`[^`\n]+`/g, ' ')
      .replace(/\[([^\[\]]*)\]\([^)]*\)/g, '$1')
      .replace(/\[\[([^\]|]+)(?:\|([^\]]*))?\]\]/g, (_, t, d) => d ?? t)
      .replace(/\s+/g, ' ')
      .trim();

    links.push({ wikiFile, path, startLine, endLine, originalText, sourceLine, lineText, headingChain, surroundingText });
  }
  return links;
}

// ── File metadata cache ───────────────────────────────────────────────────────

const fileMetaCache = new Map();

function getFileMeta(filePath, root) {
  if (fileMetaCache.has(filePath)) return fileMetaCache.get(filePath);
  let content = null;
  try { content = readFileSync(join(root, filePath), 'utf8'); } catch { /* non-existent target */ }
  const meta = { title: null, summary: null, content };
  if (content) {
    const titleMatch = content.match(/^---\s*\n(?:.*\n)*?title:\s*(.+?)\s*\n/);
    if (titleMatch) meta.title = titleMatch[1].replace(/^['"]|['"]$/g, '').trim();
    const summaryMatch = content.match(/^---\s*\n(?:.*\n)*?summary:\s*(.+?)\s*\n/);
    if (summaryMatch) meta.summary = summaryMatch[1].replace(/^['"]|['"]$/g, '').trim();
  }
  fileMetaCache.set(filePath, meta);
  return meta;
}

// ── nameRelationship ──────────────────────────────────────────────────────────

function nameRelationship(input) {
  const stop = new Set([
    'a','an','and','are','as','at','be','by','for','from','has','have','in','into',
    'is','it','its','of','on','or','that','the','their','this','to','with','without',
    'when','where','which','who','why','how','can','will','should','must','do','does',
    'not','if','then','else','new','old','true','false','null','undefined','return',
    'export','import','const','let','var','function','class','interface','type','enum',
    'public','private','protected','async','await','static','readonly',
  ]);

  const noise = new Set([
    'src','lib','app','apps','packages','pkg','server','client','common','shared',
    'components','component','utils','util','helpers','helper','services','service',
    'controllers','controller','handlers','handler','middleware','model','models',
    'schema','schemas','types','type','index','main','test','tests','spec','mock',
    'mocks','docs','doc','documentation','readme','page','file','impl','implementation',
    'deps','dependency','dependencies','link','links','api','route','routes',
  ]);

  const categories = {
    billing: ['billing','payment','payments','checkout','charge','charges','invoice','invoices','stripe'],
    auth: ['auth','authentication','authorization','login','logout','token','tokens','session','sessions','oauth','jwt'],
    experiments: ['experiment','experiments','rollout','rollouts','variant','variants','treatment','treatments','bucket','buckets','abtest'],
    platform: ['platform','infra','infrastructure','deploy','deployment','ci','build','observability','logging','metrics'],
    data: ['data','migration','migrations','warehouse','analytics','event','events','etl','sync'],
    security: ['security','threat','control','controls','risk','permission','permissions','policy','policies'],
    notifications: ['notification','notifications','email','sms','template','templates','message','messages'],
    cli: ['cli','command','commands','flag','flags','option','options','parser'],
  };

  const typeSignals = [
    {
      type: 'flow', suffix: 'flow',
      words: ['flow','request','submit','submission','route','handler','pipeline','process','carries','from','to'],
      why: (core, obj, src, tgt) => `${cap(core)} that carries ${obj} across ${src} and ${tgt}.`,
    },
    {
      type: 'contract', suffix: 'contract',
      words: ['contract','schema','interface','type','payload','request','response','shape','match','matches','parse','parses','validate','validates'],
      why: (core, obj, src, tgt) => `${cap(core)} that keeps ${src} and ${tgt} aligned on ${obj}.`,
    },
    {
      type: 'rule', suffix: 'rule',
      words: ['rule','policy','limit','limits','allowed','forbidden','validation','permission','permissions','govern','governs'],
      why: (core, obj, src, tgt) => `${cap(core)} that governs ${obj} across ${src} and ${tgt}.`,
    },
    {
      type: 'sync', suffix: 'sync',
      words: ['summary','overview','citation','cites','adr','sync','kept','documentation','docs','mirror','mirrors'],
      why: (core, obj, src, tgt) => `${cap(core)} that keeps ${src} and ${tgt} aligned around ${obj}.`,
    },
    {
      type: 'runbook', suffix: 'runbook',
      words: ['runbook','procedure','incident','p1','migration','rollback','rollout','operate','operations'],
      why: (core, obj, src, tgt) => `${cap(core)} that describes how ${obj} is performed across ${src} and ${tgt}.`,
    },
    {
      type: 'template', suffix: 'template',
      words: ['template','render','renders','email','sms','notification','message','copy'],
      why: (core, obj, src, tgt) => `${cap(core)} that defines how ${obj} is described in ${src} and rendered by ${tgt}.`,
    },
    {
      type: 'controls', suffix: 'controls',
      words: ['threat','model','mitigation','control','controls','security','risk'],
      why: (core, obj, src, tgt) => `${cap(core)} that connects ${obj} across ${src} and ${tgt}.`,
    },
    {
      type: 'config', suffix: 'config',
      words: ['config','configuration','setting','settings','env','environment','flag','option'],
      why: (core, obj, src, tgt) => `${cap(core)} that configures ${obj} across ${src} and ${tgt}.`,
    },
  ];

  const sourceText = [
    input.sourceTitle,
    ...(input.headingChain ?? []),
    input.linkText,
    input.sourcePath,
    input.surroundingText,
    ...(input.sourceTerms ?? []),
    input.sourceContentSummary,
    (input.sourceContent ?? '').slice(0, 5000),
  ].filter(Boolean).join('\n');

  const targetText = [
    input.targetTitle,
    input.targetPath,
    ...(input.targetTerms ?? []),
    input.targetContentSummary,
    (input.targetContent ?? '').slice(0, 5000),
  ].filter(Boolean).join('\n');

  const allText = `${sourceText}\n${targetText}`;
  const sourceTokens = tokenize(sourceText);
  const targetTokens = tokenize(targetText);
  const allTokens = tokenize(allText);

  const sourceCounts = countMap(sourceTokens);
  const targetCounts = countMap(targetTokens);
  const allCounts = countMap(allTokens);

  const pathTokens = tokenize(`${input.sourcePath ?? ''} ${input.targetPath ?? ''}`);
  const headingTokens = tokenize(`${input.sourceTitle ?? ''} ${(input.headingChain ?? []).join(' ')}`);
  const explicitTerms = tokenize([...(input.sourceTerms ?? []), ...(input.targetTerms ?? [])].join(' '));

  const scoredTerms = [...allCounts.keys()]
    .filter(t => t.length > 2 && !stop.has(t) && !noise.has(t) && !/^\d+$/.test(t))
    .map(term => {
      let score = allCounts.get(term);
      if (sourceCounts.has(term) && targetCounts.has(term)) score += 8;
      if (headingTokens.includes(term)) score += 5;
      if (explicitTerms.includes(term)) score += 5;
      if (pathTokens.includes(term)) score += 2;
      if (noise.has(term)) score -= 10;
      return { term, score };
    })
    .sort((a, b) => b.score - a.score);

  const topTerms = scoredTerms.slice(0, 12).map(x => x.term);
  const category = bestCategory(categories, allTokens, pathTokens, headingTokens);
  const relationship = bestRelationshipType(typeSignals, allTokens);

  const phraseCandidates = candidatePhrases(input)
    .map(normalizePhrase)
    .filter(Boolean)
    .filter(p => !isBadPhrase(p, noise))
    .map(phrase => ({
      phrase,
      score: phraseScore(phrase, sourceCounts, targetCounts, headingTokens, explicitTerms, pathTokens, topTerms)
        + (phrase.includes(relationship.suffix) ? 2 : 0),
    }))
    .sort((a, b) => b.score - a.score);

  let corePhrase = phraseCandidates[0]?.phrase;
  if (!corePhrase) corePhrase = topTerms.slice(0, 3).join(' ');
  corePhrase = ensureRelationshipSuffix(corePhrase, relationship.suffix);

  const objectPhrase =
    phraseCandidates.find(p => p.phrase !== corePhrase && !p.phrase.endsWith(relationship.suffix))?.phrase
    ?? (topTerms.slice(0, 2).join(' ') || 'the shared concern');

  const sourceRole = rolePhrase(input.sourceTitle, input.sourcePath, input.headingChain?.at(-1), 'source documentation');
  const targetRole = rolePhrase(input.targetTitle, input.targetPath, undefined, 'target implementation');

  const slug = slugify(corePhrase);
  const safeSlug = sanitizeSlug(slug);
  const name = category ? `wiki/${category}/${safeSlug}` : `wiki/${safeSlug}`;
  const why = relationship.why(corePhrase, objectPhrase || 'the shared concern', sourceRole, targetRole);

  const confidence = clamp(
    0.35
    + Math.min(0.25, phraseCandidates[0]?.score ? phraseCandidates[0].score / 80 : 0)
    + (category ? 0.1 : 0)
    + (topTerms.length >= 4 ? 0.1 : 0)
    + (sourceTokens.some(t => targetCounts.has(t) && !noise.has(t) && !stop.has(t)) ? 0.2 : 0),
    0, 0.95,
  );

  return {
    name,
    why,
    confidence,
    debug: { category, relationshipType: relationship.type, corePhrase, objectPhrase, sourceRole, targetRole, topTerms },
  };

  // ── helpers ────────────────────────────────────────────────────────────────

  function tokenize(text) {
    return text
      .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
      .replace(/[_/.\-:{}()[\],#"`'<>]+/g, ' ')
      .toLowerCase()
      .split(/\s+/)
      .map(t => t.trim())
      .filter(Boolean);
  }

  function countMap(tokens) {
    const m = new Map();
    for (const t of tokens) m.set(t, (m.get(t) ?? 0) + 1);
    return m;
  }

  function bestCategory(cats, tokens, path, headings) {
    let best;
    for (const [cat, words] of Object.entries(cats)) {
      const ws = new Set(words);
      let score = 0;
      for (const t of tokens) if (ws.has(t)) score += 1;
      for (const t of path) if (ws.has(t)) score += 2;
      for (const t of headings) if (ws.has(t)) score += 3;
      if (!best || score > best.score) best = { cat, score };
    }
    return best && best.score >= 3 ? best.cat : undefined;
  }

  function bestRelationshipType(signals, tokens) {
    let best = signals[0], bestScore = -Infinity;
    for (const sig of signals) {
      const ws = new Set(sig.words);
      let score = 0;
      for (const t of tokens) if (ws.has(t)) score += 1;
      if (score > bestScore) { best = sig; bestScore = score; }
    }
    return best;
  }

  function candidatePhrases(inp) {
    const phrases = [];

    // Seed from structured metadata — these are the most reliable signal
    phrases.push(inp.sourceTitle ?? '');
    phrases.push(inp.targetTitle ?? '');
    phrases.push(inp.linkText ?? '');
    phrases.push(...(inp.headingChain ?? []));
    phrases.push(...(inp.sourceTerms ?? []));
    phrases.push(...(inp.targetTerms ?? []));
    phrases.push(inp.sourceContentSummary ?? '');
    phrases.push(inp.targetContentSummary ?? '');

    // Path stems — useful for naming but strip directory noise
    for (const p of [inp.sourcePath, inp.targetPath]) {
      if (!p) continue;
      const base = p.split(/[\\/]/).pop() ?? p;
      phrases.push(base.replace(/\.[^.]+$/, ''));
      // Individual meaningful path segments (skip generic ones handled by noise filter)
      for (const seg of p.split(/[\\/]/)) phrases.push(seg.replace(/\.[^.]+$/, ''));
    }

    // N-grams only from surroundingText (already cleaned prose, not code)
    if (inp.surroundingText) {
      const cleaned = inp.surroundingText
        .replace(/`[^`]+`/g, ' ')
        .replace(/\[\[([^\]]+)\]\]/g, ' $1 ')
        .replace(/\[[^\]]+\]\([^)]+\)/g, ' ')
        .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
        .replace(/[^A-Za-z0-9 '-]/g, ' ');
      const words = cleaned.split(/\s+/).filter(Boolean);
      for (let n = 2; n <= 4; n++) {
        for (let idx = 0; idx <= words.length - n; idx++) {
          phrases.push(words.slice(idx, idx + n).join(' '));
        }
      }
    }

    return [...new Set(phrases)];
  }

  function normalizePhrase(phrase) {
    const tokens = tokenize(phrase).filter(t => !stop.has(t) && !/^\d+$/.test(t));
    return trimNoise(tokens).join(' ');
  }

  function trimNoise(tokens) {
    let a = [...tokens];
    while (a.length && noise.has(a[0])) a.shift();
    while (a.length && noise.has(a[a.length - 1])) a.pop();
    return a;
  }

  function isBadPhrase(phrase, bad) {
    const parts = phrase.split(/\s+/);
    if (parts.length === 0 || parts.length > 5) return true;
    if (parts.every(p => bad.has(p) || stop.has(p))) return true;
    if (parts.some(p => ['misc','temp','frontend','backend','impl','deps'].includes(p))) return true;
    return false;
  }

  function phraseScore(phrase, source, target, headings, explicit, path, top) {
    const ts = phrase.split(/\s+/);
    let score = 0;
    for (const t of ts) {
      score += source.get(t) ?? 0;
      score += target.get(t) ?? 0;
      if (source.has(t) && target.has(t)) score += 8;
      if (headings.includes(t)) score += 5;
      if (explicit.includes(t)) score += 5;
      if (path.includes(t)) score += 1;
      if (top.includes(t)) score += 3;
      if (noise.has(t)) score -= 4;
    }
    score += Math.max(0, 4 - Math.abs(3 - ts.length));
    return score;
  }

  function ensureRelationshipSuffix(phrase, suffix) {
    const parts = phrase.split(/\s+/).filter(Boolean);
    if (parts.includes(suffix) || phrase.endsWith(` ${suffix}`)) return phrase;
    if (['rate limits','auth token'].includes(phrase)) return phrase;
    const last = parts[parts.length - 1];
    if (['flow','contract','rule','sync','runbook','template','controls','config','rollout'].includes(last)) return phrase;
    return `${phrase} ${suffix}`;
  }

  function rolePhrase(title, path, heading, fallback) {
    const raw = title || heading || path?.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, '') || fallback;
    const phrase = normalizePhrase(raw);
    if (!phrase) return fallback;
    if (path?.match(/\.(md|mdx|txt|rst)$/i) || fallback.includes('documentation')) return `${phrase} documentation`;
    return phrase;
  }

  function slugify(phrase) {
    return normalizePhrase(phrase)
      .split(/\s+/)
      .filter(t => !['ts','tsx','js','jsx','py','rs','go','md','mdx'].includes(t))
      .join('-');
  }

  function sanitizeSlug(slug) {
    let s = slug
      .replace(/-+/g, '-').replace(/^-|-$/g, '')
      .replace(/\b(misc|temp|john-work)\b/g, '')
      .replace(/-+/g, '-').replace(/^-|-$/g, '');
    for (const bad of ['-deps','-impl','-file','-doc','-link']) {
      if (s.endsWith(bad)) s = s.slice(0, -bad.length);
    }
    return s || 'relationship';
  }

  function cap(s) { return s ? s[0].toUpperCase() + s.slice(1) : s; }
  function clamp(n, min, max) { return Math.max(min, Math.min(max, n)); }
}

// ── Frontmatter helpers ───────────────────────────────────────────────────────

function readFrontmatterTitle(filePath) {
  try {
    const content = readFileSync(filePath, 'utf8');
    const m = content.match(/^---\s*\n(?:.*\n)*?title:\s*(.+?)\s*\n/);
    return m ? m[1].replace(/^['"]|['"]$/g, '').trim() : null;
  } catch { return null; }
}

// ── Deduplication ─────────────────────────────────────────────────────────────

function deduplicateNames(meshes) {
  const counts = new Map();
  for (const m of meshes) counts.set(m.name, (counts.get(m.name) ?? 0) + 1);
  const seen = new Map();
  for (const m of meshes) {
    if ((counts.get(m.name) ?? 1) > 1) {
      const n = (seen.get(m.name) ?? 0) + 1;
      seen.set(m.name, n);
      m.name = `${m.name}-${n}`;
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
  allLinks.push(...parseFragmentLinks(content, file));
}

if (allLinks.length === 0) {
  console.error('No fragment links with line ranges found.');
  process.exit(0);
}

const wikiFileTitle = new Map();
const wikiFileSummary = new Map();
for (const file of files) {
  const title = readFrontmatterTitle(file);
  if (title) wikiFileTitle.set(file, title);
  try {
    const content = readFileSync(file, 'utf8');
    const m = content.match(/^---\s*\n(?:.*\n)*?summary:\s*(.+?)\s*\n/);
    if (m) wikiFileSummary.set(file, m[1].replace(/^['"]|['"]$/g, '').trim());
  } catch { /* ok */ }
}

const meshes = allLinks.map(link => {
  const targetMeta = getFileMeta(link.path, root);

  // Scope targetContent to the anchored lines only — the whole file floods
  // term scoring with unrelated tokens from surrounding code.
  let targetSnippet = null;
  if (targetMeta.content) {
    const lines = targetMeta.content.split('\n');
    const snipStart = Math.max(0, link.startLine - 1);
    const snipEnd = Math.min(lines.length, link.endLine + 5);
    targetSnippet = lines.slice(snipStart, snipEnd).join('\n');
  }

  const result = nameRelationship({
    sourceTitle: wikiFileTitle.get(link.wikiFile),
    sourceContentSummary: wikiFileSummary.get(link.wikiFile),
    targetTitle: targetMeta.title,
    targetContentSummary: targetMeta.summary,
    targetContent: targetSnippet,
    linkText: link.originalText.replace(/[`*_[\]]/g, '').trim(),
    sourcePath: relative(root, link.wikiFile).replace(/\\/g, '/'),
    targetPath: link.path,
    headingChain: link.headingChain,
    surroundingText: link.surroundingText,
  });

  return {
    name: result.name,
    why: result.why,
    confidence: result.confidence,
    debug: result.debug,
    wikiFile: relative(root, link.wikiFile).replace(/\\/g, '/'),
    anchor: `${link.path}#L${link.startLine}-L${link.endLine}`,
  };
});

deduplicateNames(meshes);

// ── Output ────────────────────────────────────────────────────────────────────

if (outputFormat === 'json') {
  console.log(JSON.stringify(meshes, null, 2));
  process.exit(0);
}

const byFile = new Map();
for (const m of meshes) {
  if (!byFile.has(m.wikiFile)) byFile.set(m.wikiFile, []);
  byFile.get(m.wikiFile).push(m);
}

console.log('#!/bin/sh');
console.log('# Generated by scripts/mesh-scaffold-v2.mjs');
console.log('# Review mesh names and whys before running.');
console.log('# Commit each mesh: git mesh commit <name>');
console.log();

for (const [wikiFile, entries] of byFile) {
  console.log(`# ── ${wikiFile} ${'─'.repeat(Math.max(0, 60 - wikiFile.length))}`);
  for (const m of entries) {
    if (showDebug) {
      console.log(`# confidence=${m.confidence.toFixed(2)} type=${m.debug.relationshipType} category=${m.debug.category ?? 'none'}`);
      console.log(`# core="${m.debug.corePhrase}" object="${m.debug.objectPhrase}"`);
      console.log(`# terms=[${m.debug.topTerms.slice(0, 6).join(', ')}]`);
    }
    console.log();
    console.log(`git mesh add ${m.name} \\`);
    console.log(`  ${m.wikiFile} \\`);
    console.log(`  ${m.anchor}`);
    console.log(`git mesh why ${m.name} -m "${m.why}"`);
  }
  console.log();
}
