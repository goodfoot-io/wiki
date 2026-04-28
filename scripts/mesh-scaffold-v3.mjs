#!/usr/bin/env node
/**
 * mesh-scaffold-v3.mjs
 *
 * Improvements over v2:
 *
 * NAMES
 *  - RAKE on surroundingText replaces n-gram-from-all-text; candidate pool is
 *    purely local prose, eliminating source-title domination
 *  - Source title tokens removed from phrase candidates (kept only in freq maps)
 *  - Individual path segments no longer pushed as candidates (only file stem)
 *  - Proper nouns preserved before camelCase split (WorkOS→workos, JWT→jwt)
 *  - Target role derived by walking up path past noise segments (never "target implementation")
 *  - ensureRelationshipSuffix removed; suffix only added when name would be ambiguous
 *  - Relationship type: specific signal words only, positive threshold, default=sync
 *
 * WHYS
 *  - Hybrid: prose extraction (v1 approach, improved) primary; template fallback
 *  - Template uses RAKE corePhrase + target path role (not source title)
 *  - objectPhrase drawn from target context (title, summary, path), not source
 *  - Source role = deepest heading, not full page title
 *  - Template "sync" type as default (wiki→code is always documentation coverage)
 */

import { readFileSync, readdirSync, statSync } from 'fs';
import { join, relative, extname, basename } from 'path';

const args = process.argv.slice(2);
const outputFormat = args.includes('--output') ? args[args.indexOf('--output') + 1] : 'shell';
const showDebug = args.includes('--show-debug');

// ── File discovery ─────────────────────────────────────────────────────────────

const EXCLUDE_DIRS = new Set(['node_modules', '.git', '.claude', 'plugins', 'npm', 'target', '.worktrees']);

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
      if (stat.isDirectory()) { walk(full, inWikiDir || entry === 'wiki'); }
      else if (entry.endsWith('.wiki.md')) { results.push(full); }
      else if (inWikiDir && entry.endsWith('.md')) { results.push(full); }
    }
  }
  walk(root, false);
  return results;
}

// ── Markdown scrubbing ─────────────────────────────────────────────────────────

function scrubNonContent(content) {
  const chars = content.split('');
  const len = chars.length;
  let i = 0;
  function blank(s, e) { for (let j = s; j < e; j++) if (chars[j] !== '\n') chars[j] = ' '; }
  while (i < len) {
    const rest = content.slice(i);
    if (rest.startsWith('<!--')) {
      const end = content.indexOf('-->', i + 4);
      if (end !== -1) { blank(i, end + 3); i = end + 3; } else { blank(i, len); i = len; }
      continue;
    }
    if ((i === 0 || content[i - 1] === '\n') && (rest.startsWith('```') || rest.startsWith('~~~'))) {
      const fc = content[i]; let fl = 3;
      while (i + fl < len && content[i + fl] === fc) fl++;
      const fence = content.slice(i, i + fl), bs = i;
      let j = i + fl;
      while (j < len && content[j] !== '\n') j++;
      if (j < len) j++;
      let found = false;
      while (j < len) {
        if (content.slice(j).startsWith(fence)) {
          let k = j + fl;
          while (k < len && content[k] === ' ') k++;
          if (k >= len || content[k] === '\n') { blank(bs, k < len ? k + 1 : k); i = k < len ? k + 1 : k; found = true; break; }
        }
        while (j < len && content[j] !== '\n') j++;
        if (j < len) j++;
      }
      if (!found) { blank(bs, len); i = len; }
      continue;
    }
    if (content[i] === '`') {
      let tc = 1;
      while (i + tc < len && content[i + tc] === '`') tc++;
      if (tc < 3) { const end = content.indexOf('`'.repeat(tc), i + tc); if (end !== -1) { blank(i + tc, end); i = end + tc; continue; } }
      else { i += tc; continue; }
    }
    i++;
  }
  return chars.join('');
}

// ── Fragment link + heading parser ─────────────────────────────────────────────

const MD_LINK_RE = /\[([^\[\]]*)\]\(([^)]*)\)/g;
const URL_SCHEME_RE = /^[a-zA-Z][a-zA-Z0-9+\-.]*:\/\//;
const LINE_RANGE_RE = /^L(\d+)(?:-L?(\d+))?(?:&[0-9a-f]+)?$/i;
const HEADING_RE = /^(#{1,6})\s+(.+)/;

function parseFragmentLinks(content, wikiFile) {
  const scrubbed = scrubNonContent(content);
  const lines = content.split('\n');
  const links = [];

  const headingStack = [];
  const headingChainByLine = [];
  for (const line of lines) {
    const hm = HEADING_RE.exec(line.trimStart());
    if (hm) {
      const level = hm[1].length;
      const text = hm[2].replace(/[`*_[\]]/g, '').trim();
      while (headingStack.length && headingStack[headingStack.length - 1].level >= level) headingStack.pop();
      headingStack.push({ level, text });
    }
    headingChainByLine.push(headingStack.map(h => h.text));
  }

  for (const match of scrubbed.matchAll(MD_LINK_RE)) {
    const [, , href] = match;
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

    // 3 lines of context for prose extraction
    const surroundingText = lines
      .slice(Math.max(0, sourceLine - 3), sourceLine + 2)
      .join(' ')
      .replace(/`[^`\n]+`/g, ' ')
      .replace(/\[([^\[\]]*)\]\([^)]*\)/g, '$1')
      .replace(/\[\[([^\]|]+)(?:\|([^\]]*))?\]\]/g, (_, t, d) => d ?? t)
      .replace(/\s+/g, ' ').trim();

    links.push({ wikiFile, path, startLine, endLine, originalText, sourceLine, lineText, headingChain, surroundingText });
  }
  return links;
}

// ── File metadata cache ────────────────────────────────────────────────────────

const fileMetaCache = new Map();
function getFileMeta(filePath, root) {
  if (fileMetaCache.has(filePath)) return fileMetaCache.get(filePath);
  let content = null;
  try { content = readFileSync(join(root, filePath), 'utf8'); } catch { /* ok */ }
  const meta = { title: null, summary: null, content };
  if (content) {
    const tm = content.match(/^---\s*\n(?:.*\n)*?title:\s*(.+?)\s*\n/);
    if (tm) meta.title = tm[1].replace(/^['"]|['"]$/g, '').trim();
    const sm = content.match(/^---\s*\n(?:.*\n)*?summary:\s*(.+?)\s*\n/);
    if (sm) meta.summary = sm[1].replace(/^['"]|['"]$/g, '').trim();
  }
  fileMetaCache.set(filePath, meta);
  return meta;
}

// ── Tokenization with proper noun preservation ────────────────────────────────

function normalizeProperNouns(text) {
  return text
    // Compound proper nouns: WorkOS, GitHub, AuthKit, OpenID → lowercase-atomically
    .replace(/\b([A-Z][a-z]+[A-Z][A-Za-z]*)\b/g, m => m.toLowerCase())
    // Acronyms: JWT, API, URL, OAuth, CI → lowercase (prevents splitting later)
    .replace(/\b[A-Z]{2,6}\b/g, m => m.toLowerCase());
}

const STOP = new Set([
  'a','an','and','are','as','at','be','by','for','from','has','have','in','into',
  'is','it','its','of','on','or','that','the','their','this','to','with','without',
  'when','where','which','who','why','how','can','will','should','must','do','does',
  'not','if','then','else','new','old','true','false','null','undefined','return',
  'export','import','const','let','var','function','class','interface','type','enum',
  'public','private','protected','async','await','static','readonly','each','also',
  'after','before','using','used','via','through','across','within','between',
]);

const NOISE = new Set([
  'src','lib','app','apps','packages','pkg','server','client','common','shared',
  'components','component','utils','util','helpers','helper','services','service',
  'controllers','controller','handlers','handler','middleware','model','models',
  'schema','schemas','types','type','index','main','test','tests','spec','mock',
  'mocks','docs','doc','documentation','readme','page','file','impl','implementation',
  'deps','dependency','dependencies','link','links','api','route','routes',
]);

function tokenize(text) {
  return normalizeProperNouns(text)
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .replace(/[_/.\-:{}()[\],#"`'<>|=+*!?@$%^&~]+/g, ' ')
    .toLowerCase()
    .split(/\s+/)
    .map(t => t.trim())
    .filter(t => t.length > 1 && !/^\d+$/.test(t));
}

function countMap(tokens) {
  const m = new Map();
  for (const t of tokens) m.set(t, (m.get(t) ?? 0) + 1);
  return m;
}

// ── RAKE keyword extraction ────────────────────────────────────────────────────
// Extracts scored candidate phrases from prose text without an external corpus.
// Key insight: words in long multi-word phrases get higher degree scores than
// frequency, rewarding specific compound terms over generic single words.

function rake(text) {
  if (!text) return [];

  // Split into candidate phrases at stop words and punctuation boundaries
  const stopPattern = new RegExp(
    `\\b(${[...STOP].map(w => w.replace(/[-.*+?^${}()|[\]\\]/g, '\\$&')).join('|')})\\b|[^a-zA-Z0-9'\\-]+`,
    'gi'
  );
  const normalized = normalizeProperNouns(text).toLowerCase();
  const parts = normalized.split(stopPattern)
    .map(p => (p ?? '').trim())
    .filter(p => p && /[a-z]/.test(p));

  // Build candidate phrases (1–4 word spans from each part)
  const candidates = [];
  for (const part of parts) {
    const words = part.split(/\s+/).filter(w => w.length > 1 && !/^\d+$/.test(w) && !NOISE.has(w));
    if (words.length === 0) continue;
    // All contiguous sub-spans up to length 4
    for (let start = 0; start < words.length; start++) {
      for (let len = 1; len <= Math.min(4, words.length - start); len++) {
        candidates.push(words.slice(start, start + len));
      }
    }
  }

  if (candidates.length === 0) return [];

  // RAKE word scoring: score(w) = deg(w) / freq(w)
  const wordFreq = new Map();
  const wordDeg = new Map();
  for (const phrase of candidates) {
    for (const word of phrase) {
      wordFreq.set(word, (wordFreq.get(word) ?? 0) + 1);
      wordDeg.set(word, (wordDeg.get(word) ?? 0) + phrase.length);
    }
  }
  const wordScore = new Map();
  for (const [w, freq] of wordFreq) {
    wordScore.set(w, (wordDeg.get(w) ?? 0) / freq);
  }

  // Score phrases, deduplicate
  const seen = new Set();
  const scored = [];
  for (const phrase of candidates) {
    const key = phrase.join(' ');
    if (seen.has(key)) continue;
    seen.add(key);
    if (phrase.every(w => STOP.has(w) || NOISE.has(w))) continue;
    const score = phrase.reduce((sum, w) => sum + (wordScore.get(w) ?? 0), 0);
    scored.push({ phrase: key, words: phrase, score });
  }

  return scored.sort((a, b) => b.score - a.score);
}

// ── Cross-document co-presence ────────────────────────────────────────────────
// Find terms that appear in BOTH source context and target context.
// These are the conceptual glue of the relationship.

function coPresenceTerms(srcText, tgtText) {
  const srcTokens = tokenize(srcText);
  const tgtTokens = tokenize(tgtText);
  if (srcTokens.length === 0 || tgtTokens.length === 0) return [];

  const srcCounts = countMap(srcTokens);
  const tgtCounts = countMap(tgtTokens);
  const srcTotal = srcTokens.length;
  const tgtTotal = tgtTokens.length;

  const result = [];
  for (const [t, sc] of srcCounts) {
    if (STOP.has(t) || NOISE.has(t) || t.length < 3 || /^\d+$/.test(t)) continue;
    const tc = tgtCounts.get(t) ?? 0;
    if (tc === 0) continue;
    const tfSrc = sc / srcTotal;
    const tfTgt = tc / tgtTotal;
    result.push({ term: t, score: Math.min(tfSrc, tfTgt) * (sc + tc) });
  }
  return result.sort((a, b) => b.score - a.score);
}

// ── Target role extraction ─────────────────────────────────────────────────────
// Walk up the target path to find the first non-noise, non-generic segment.
// This prevents "target implementation" fallbacks.

function extractTargetRole(targetPath, targetTitle) {
  if (targetTitle) {
    const t = tokenize(targetTitle).filter(w => !STOP.has(w) && !NOISE.has(w));
    if (t.length > 0) return t.slice(0, 3).join(' ');
  }
  if (!targetPath) return 'target';
  const segments = targetPath.split('/').map(s => s.replace(/\.[^.]+$/, '').toLowerCase());
  // Walk from filename back up through directories, skip noise
  for (let i = segments.length - 1; i >= 0; i--) {
    const seg = segments[i];
    if (!NOISE.has(seg) && !STOP.has(seg) && seg.length > 2 && !/^\d+$/.test(seg)) {
      return seg.replace(/[-_]/g, ' ');
    }
  }
  return segments[segments.length - 1] || 'target';
}

// ── Source role extraction ─────────────────────────────────────────────────────
// Use the most specific (deepest) heading rather than the full page title.

function extractSourceRole(headingChain, sourceTitle) {
  // Most specific heading that is not noise-only
  for (let i = headingChain.length - 1; i >= 0; i--) {
    const h = headingChain[i];
    const tokens = tokenize(h).filter(w => !STOP.has(w) && !NOISE.has(w));
    if (tokens.length > 0) return tokens.slice(0, 4).join(' ');
  }
  if (sourceTitle) {
    const tokens = tokenize(sourceTitle).filter(w => !STOP.has(w) && !NOISE.has(w));
    if (tokens.length > 0) return tokens.slice(0, 3).join(' ');
  }
  return 'source';
}

// ── Relationship type detection ────────────────────────────────────────────────
// Stricter signals, positive threshold, default=sync (wiki→code is always docs).

const REL_TYPES = [
  {
    type: 'contract', suffix: 'contract', threshold: 2,
    words: new Set(['schema','payload','shape','validates','parses','matches','expects','contract','format','structure','serializes','deserializes']),
    why: (core, obj, src, tgt) => `${cap(core)} that synchronizes the ${obj} shape expected by ${src} with what ${tgt} provides.`,
  },
  {
    type: 'rule', suffix: 'rule', threshold: 2,
    words: new Set(['validates','enforces','governs','policy','forbidden','allowed','permission','guard','boundary','invariant','constraint','rejects','denies']),
    why: (core, obj, src, tgt) => `${cap(core)} that enforces the ${obj} constraint across ${src} and ${tgt}.`,
  },
  {
    type: 'flow', suffix: 'flow', threshold: 3,
    words: new Set(['submits','routes','dispatches','pipeline','webhook','endpoint','handler','request','response','emits','propagates','triggers','subscribes']),
    why: (core, obj, src, tgt) => `${cap(core)} that routes ${obj} from ${src} through ${tgt}.`,
  },
  {
    type: 'config', suffix: 'config', threshold: 2,
    words: new Set(['config','configuration','settings','env','environment','wrangler','variable','flag','option','binding','secret','deploy']),
    why: (core, obj, src, tgt) => `${cap(core)} that wires the ${obj} configuration from ${src} into ${tgt}.`,
  },
  {
    // Default: wiki page documents the code — always true for wiki→code anchors
    type: 'sync', suffix: 'sync', threshold: 0,
    words: new Set([]),
    why: (core, obj, src, tgt) => `${cap(core)} in ${src} that covers the ${obj} implementation in ${tgt}.`,
  },
];

function detectRelType(allTokens) {
  for (const rel of REL_TYPES.slice(0, -1)) {
    let score = 0;
    for (const t of allTokens) if (rel.words.has(t)) score++;
    if (score >= rel.threshold) return rel;
  }
  return REL_TYPES[REL_TYPES.length - 1]; // sync
}

// ── Category detection ─────────────────────────────────────────────────────────

const CATEGORIES = {
  billing: ['billing','payment','payments','checkout','charge','stripe','invoice','subscription'],
  auth: ['auth','authentication','authorization','login','logout','token','session','oauth','jwt','workos','authkit'],
  experiments: ['experiment','rollout','variant','treatment','bucket','abtest','flag','feature'],
  platform: ['platform','infra','infrastructure','deploy','deployment','ci','build','observability','logging','metrics'],
  data: ['data','migration','warehouse','analytics','event','etl','sync','database'],
  security: ['security','threat','mitigation','control','risk','permission','policy','compliance'],
  notifications: ['notification','email','sms','template','message','webhook'],
  cli: ['cli','command','parser','flag','option','repl','stdin','stdout'],
};

function detectCategory(allTokens, pathTokens, headingTokens) {
  let best;
  for (const [cat, words] of Object.entries(CATEGORIES)) {
    const ws = new Set(words);
    let score = 0;
    for (const t of allTokens) if (ws.has(t)) score += 1;
    for (const t of pathTokens) if (ws.has(t)) score += 2;
    for (const t of headingTokens) if (ws.has(t)) score += 3;
    if (!best || score > best.score) best = { cat, score };
  }
  return best && best.score >= 3 ? best.cat : null;
}

// ── Core phrase selection ──────────────────────────────────────────────────────
// Priority: RAKE from surroundingText → co-presence terms → link label → path stem.
// Source title tokens are EXCLUDED from candidacy.

function selectCorePhrase(rakeResults, coPresent, linkText, targetPath, sourceTitleTokens) {
  const exclude = sourceTitleTokens;

  // Helper: is a phrase dominated by source title tokens?
  function titleDominated(phrase) {
    const words = phrase.split(/\s+/).filter(Boolean);
    if (words.length === 0) return true;
    const titleOverlap = words.filter(w => exclude.has(w)).length / words.length;
    return titleOverlap > 0.5;
  }

  // 1. RAKE results — pick best phrase not dominated by source title
  for (const { phrase } of rakeResults) {
    if (!titleDominated(phrase) && phrase.length > 2) return phrase;
  }

  // 2. Co-present terms (shared between source context and target)
  if (coPresent.length > 0) {
    const top = coPresent.slice(0, 3).map(x => x.term).join(' ');
    if (!titleDominated(top) && top.length > 2) return top;
  }

  // 3. Link label (cleaned)
  if (linkText) {
    const cleaned = tokenize(linkText).filter(w => !STOP.has(w) && !NOISE.has(w)).slice(0, 4).join(' ');
    if (cleaned && !titleDominated(cleaned)) return cleaned;
  }

  // 4. Target file stem
  if (targetPath) {
    const seg = extractTargetRole(targetPath, null);
    if (seg && seg !== 'target') return seg;
  }

  // 5. Last resort: top RAKE regardless of title overlap
  return rakeResults[0]?.phrase ?? linkText ?? 'relationship';
}

// ── Prose why extraction (hybrid primary) ────────────────────────────────────
// Attempts to extract a clean why from the prose sentence containing the link.
// Falls back to the template if the result is degenerate.

function extractProseWhy(link) {
  const raw = link.lineText.trimStart();

  // Table rows: extract the description cell (longest non-path cell)
  if (raw.startsWith('|')) {
    const cells = raw.split('|')
      .map(c => c
        .replace(/`[^`\n]+`/g, '')
        .replace(/\[([^\[\]]*)\]\([^)]*\)/g, '$1')
        .replace(/\*\*([^*]+)\*\*/g, '$1')
        .replace(/\*([^*]+)\*/g, '$1')
        .trim()
      )
      .filter(c => c.length > 0);
    const desc = cells
      .filter(c => !/^[A-Za-z0-9_\-/.]+\.[a-z]{1,5}$/.test(c) && !/^`/.test(c))
      .sort((a, b) => b.length - a.length)[0];
    if (desc && desc.replace(/[.,;: ]/g, '').length >= 8) {
      return desc.replace(/[.,;: ]+$/, '') + '.';
    }
    return null;
  }

  // Headings: no prose available
  if (raw.startsWith('#')) return null;

  // Prose line
  let prose = raw
    .replace(/`[^`\n]+`/g, '')
    .replace(/\[([^\[\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[\[([^\]|]+)(?:\|([^\]]*))?\]\]/g, (_, t, d) => d ?? t)
    .replace(/^[#\-*0-9.> ]+/, '')
    .replace(/\*\*([^*]+)\*\*/g, '$1')
    .replace(/\*([^*]+)\*/g, '$1')
    .trim();

  // Clean up stripping artifacts
  prose = prose
    .replace(/\([^)]*\)/g, '')
    .replace(/\*+/g, '')
    .replace(/,(\s*,)+/g, ',')
    .replace(/\s+/g, ' ')
    .replace(/\s+([.,;:])/g, '$1')
    .replace(/^[,;:\s—–-]+/, '') // strip leading —, –, commas
    .replace(/\b(for|to|in|at|of|by|from|with|and|or)\s*\.$/, '.')
    .trim();

  // Truncate at first sentence boundary
  const sentEnd = prose.search(/[.!?]/);
  if (sentEnd !== -1) {
    prose = prose.slice(0, sentEnd + 1);
  } else if (prose.length > 160) {
    prose = prose.slice(0, 160).replace(/\s\S*$/, '') + '.';
  } else if (prose && !prose.endsWith('.')) {
    prose += '.';
  }

  // Fix headless predicates: subject was a stripped code span
  const HEADLESS = /^(is|are|was|were|applies|reserves|decides|builds|exports|stores|handles|manages|validates|parses|wraps|maps|tracks|owns|uses|caches|returns|emits|reads|writes|checks|runs|renders|sends|receives|creates|updates|deletes|fetches|loads|saves|generates|computes|resolves|detects|scans|enforces|processes|dispatches|routes)\b/i;
  if (HEADLESS.test(prose)) {
    const label = link.originalText.replace(/[`*_[\]]/g, '').trim();
    if (label && !/^[A-Za-z0-9_\-/.]+\.[a-z]{1,5}$/.test(label)) {
      prose = cap(label) + ' ' + prose[0].toLowerCase() + prose.slice(1);
    }
  }

  // Reject degenerate results
  const content = prose.replace(/[.,;:\s]/g, '');
  if (content.length < 10) return null;
  if (/^[A-Za-z0-9_\-/.]+\.[a-z]{1,5}\.?$/.test(prose.trim())) return null;
  if (/^[A-Za-z0-9_-]+\/[A-Za-z0-9_./-]+\.?$/.test(prose.trim())) return null;

  return prose;
}

// ── Template why ──────────────────────────────────────────────────────────────

function templateWhy(relType, corePhrase, objectPhrase, sourceRole, targetRole) {
  return relType.why(corePhrase, objectPhrase, sourceRole, targetRole);
}

// ── Slug helpers ───────────────────────────────────────────────────────────────

function slugify(phrase) {
  return phrase
    .replace(/[^a-z0-9 ]/gi, ' ')
    .toLowerCase()
    .split(/\s+/)
    .filter(t => t.length > 1 && !['ts','tsx','js','jsx','rs','go','md','mdx'].includes(t))
    .join('-')
    .replace(/-+/g, '-')
    .replace(/^-|-$/g, '')
    || 'relationship';
}

function cap(s) { return s ? s[0].toUpperCase() + s.slice(1) : s; }

// ── Main mesh generation ───────────────────────────────────────────────────────

function generateMesh(link, sourceTitle, sourceContentSummary, targetMeta, root) {
  const sourceTitleTokens = new Set(tokenize(sourceTitle ?? ''));

  // Anchored lines only (prevent whole-file token flooding)
  let targetSnippet = null;
  if (targetMeta.content) {
    const lines = targetMeta.content.split('\n');
    targetSnippet = lines.slice(Math.max(0, link.startLine - 1), Math.min(lines.length, link.endLine + 5)).join('\n');
  }

  // Source context: local prose + headings (NOT the full page content)
  const sourceCtx = [link.surroundingText, ...link.headingChain].join(' ');
  // Target context: snippet + title + summary
  const targetCtx = [targetSnippet, targetMeta.title, targetMeta.summary].filter(Boolean).join(' ');

  const rakeResults = rake(link.surroundingText ?? '');
  const coPresent = coPresenceTerms(sourceCtx, targetCtx);

  const linkText = link.originalText.replace(/[`*_[\]]/g, '').trim();
  const corePhrase = selectCorePhrase(rakeResults, coPresent, linkText, link.path, sourceTitleTokens);

  const allTokens = tokenize(`${sourceCtx} ${targetCtx}`);
  const pathTokens = tokenize(`${relative(root, link.wikiFile).replace(/\\/g, '/')} ${link.path}`);
  const headingTokens = tokenize([sourceTitle, ...link.headingChain].join(' '));

  const relType = detectRelType(allTokens);
  const category = detectCategory(allTokens, pathTokens, headingTokens);

  const targetRole = extractTargetRole(link.path, targetMeta.title);
  const sourceRole = extractSourceRole(link.headingChain, sourceTitle);

  // objectPhrase: from target context, not source title pool
  const objectPhrase = (() => {
    if (coPresent.length > 0) {
      const candidate = coPresent.find(x => !sourceTitleTokens.has(x.term));
      if (candidate) return candidate.term;
    }
    if (targetMeta.title) {
      const tokens = tokenize(targetMeta.title).filter(w => !STOP.has(w) && !NOISE.has(w));
      if (tokens.length > 0) return tokens.slice(0, 3).join(' ');
    }
    return targetRole;
  })();

  // Why: prose primary, template fallback
  const proseWhy = extractProseWhy(link);
  const why = proseWhy ?? templateWhy(relType, corePhrase, objectPhrase, sourceRole, targetRole);

  // Name: wiki/<category>/<coreSlug>
  const coreSlug = slugify(corePhrase);
  const name = category ? `wiki/${category}/${coreSlug}` : `wiki/${coreSlug}`;

  const wikiFile = relative(root, link.wikiFile).replace(/\\/g, '/');

  return {
    name,
    why,
    wikiFile,
    anchor: `${link.path}#L${link.startLine}-L${link.endLine}`,
    debug: showDebug ? {
      category, relType: relType.type, corePhrase, objectPhrase, sourceRole, targetRole,
      rakeTop: rakeResults.slice(0, 3).map(r => `${r.phrase}(${r.score.toFixed(1)})`),
      coPresent: coPresent.slice(0, 3).map(x => x.term),
      proseWhy: proseWhy ? '✓' : '✗ (template)',
    } : undefined,
  };
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
}

// ── Entry point ────────────────────────────────────────────────────────────────

const root = process.cwd();
const files = findWikiFiles(root);
if (files.length === 0) { console.error('No wiki files found.'); process.exit(1); }

const allLinks = [];
for (const file of files) {
  const content = readFileSync(file, 'utf8');
  allLinks.push(...parseFragmentLinks(content, file));
}
if (allLinks.length === 0) { console.error('No fragment links found.'); process.exit(0); }

const wikiMeta = new Map();
for (const file of files) {
  const meta = getFileMeta(relative(root, file), root);
  wikiMeta.set(file, { title: meta.title, summary: meta.summary });
}

const meshes = allLinks.map(link => {
  const src = wikiMeta.get(link.wikiFile) ?? {};
  const tgt = getFileMeta(link.path, root);
  return generateMesh(link, src.title, src.summary, tgt, root);
});

deduplicateNames(meshes);

// ── Output ────────────────────────────────────────────────────────────────────

if (outputFormat === 'json') { console.log(JSON.stringify(meshes, null, 2)); process.exit(0); }

const byFile = new Map();
for (const m of meshes) {
  if (!byFile.has(m.wikiFile)) byFile.set(m.wikiFile, []);
  byFile.get(m.wikiFile).push(m);
}

console.log('#!/bin/sh');
console.log('# Generated by scripts/mesh-scaffold-v3.mjs');
console.log('# Review names and whys before running. Commit: git mesh commit <name>');
console.log();

for (const [wikiFile, entries] of byFile) {
  console.log(`# ── ${wikiFile} ${'─'.repeat(Math.max(0, 60 - wikiFile.length))}`);
  for (const m of entries) {
    if (showDebug && m.debug) {
      console.log(`# [${m.debug.relType}${m.debug.category ? '/' + m.debug.category : ''}] prose=${m.debug.proseWhy}`);
      console.log(`# rake=[${m.debug.rakeTop.join(', ')}] co=[${m.debug.coPresent.join(', ')}]`);
      console.log(`# core="${m.debug.corePhrase}" obj="${m.debug.objectPhrase}" src="${m.debug.sourceRole}" tgt="${m.debug.targetRole}"`);
    }
    console.log();
    console.log(`git mesh add ${m.name} \\`);
    console.log(`  ${m.wikiFile} \\`);
    console.log(`  ${m.anchor}`);
    console.log(`git mesh why ${m.name} -m "${m.why}"`);
  }
  console.log();
}
