#!/usr/bin/env node
/**
 * mesh-scaffold-v4.mjs
 *
 * Improvements over v3:
 *
 * NAMES
 *  - File extensions (ts, tsx, js, rs…) filtered from RAKE/tokenize to prevent
 *    "config ts", "extension extension ts" phrase contamination
 *  - 'references', 'sources', 'entries' added to NOISE (eliminated "X in references")
 *  - Link labels in surroundingText: file extension stripped before RAKE runs
 *
 * WHYS
 *  - Prose cleanup: arrow chars (→, —), L\d+ line refs stripped before validation
 *  - Orphaned punctuation: patterns like "by, by:", ":,.", "in:." cleaned after
 *    backtick stripping (handles "driven by, which is mutated by and rebuilt by:.")
 *  - Heading-label rejection: prose that ends with ":" before the period (i.e.
 *    bold-formatted labels like "Runtime Configuration:") returns null → template
 *  - Trailing-preposition rejection: "... by." "... in." "... from." → null
 *  - Short why rejection: raised to 20 real chars (catches "Declared at.", "L344 is.")
 *  - Path-in-why rejection: prose containing a path token (word/word) → null
 *
 * TEMPLATES
 *  - Sync template redesigned: "Name the subsystem, say what it does" (handbook)
 *    instead of "X in Y that covers Z in Z" (location description)
 *  - When obj == tgt (tautological): uses simpler "X implementation, as documented
 *    in the Y wiki section" form
 *  - Other rel types: objectPhrase replaced with specific target role when obj == tgt
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

    // 3 lines of context; strip file extension from link labels to prevent RAKE
    // from seeing "extension ts" or "config ts" as phrase candidates.
    const surroundingText = lines
      .slice(Math.max(0, sourceLine - 3), sourceLine + 2)
      .join(' ')
      .replace(/`[^`\n]+`/g, ' ')
      .replace(/\[([^\[\]]*)\]\([^)]*\)/g, (_, label) => label.replace(/\.[a-z]{1,5}$/, ''))
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

// ── Tokenization ──────────────────────────────────────────────────────────────

function normalizeProperNouns(text) {
  return text
    .replace(/\b([A-Z][a-z]+[A-Z][A-Za-z]*)\b/g, m => m.toLowerCase())
    .replace(/\b[A-Z]{2,6}\b/g, m => m.toLowerCase());
}

const FILE_EXTS = new Set([
  'ts','tsx','js','jsx','mjs','cjs','rs','go','py','rb','swift',
  'css','scss','html','json','toml','yaml','yml','md','mdx','sh','bash','env',
]);

const STOP = new Set([
  'a','an','and','are','as','at','be','by','for','from','has','have','in','into',
  'is','it','its','of','on','or','that','the','their','this','to','with','without',
  'when','where','which','who','why','how','can','will','should','must','do','does',
  'not','if','then','else','new','old','true','false','null','undefined','return',
  'export','import','const','let','var','function','class','interface','type','enum',
  'public','private','protected','async','await','static','readonly','each','also',
  'after','before','using','used','via','through','across','within','between',
  // pronouns/quantifiers that slip through as core phrases
  'all','what','these','those','some','any','use','get','set','both','every',
  // adverbs and qualifiers that are never subsystem names
  'exactly','strictly','currently','initially','simply','properly','correctly',
  'already','still','never','always','often','always','only','just','more','less',
  'either','neither','inside','outside','above','below','here','there','same',
]);

const NOISE = new Set([
  'src','lib','app','apps','packages','pkg','server','client','common','shared',
  'components','component','utils','util','helpers','helper','services','service',
  'controllers','controller','handlers','handler','middleware','model','models',
  'schema','schemas','types','type','index','main','test','tests','spec','mock',
  'mocks','docs','doc','documentation','readme','page','file','impl','implementation',
  'deps','dependency','dependencies','link','links','api','route','routes',
  // v4 additions: paths/refs that produce tautological templates
  'references','reference','sources','source','entries','entry',
  'piece','barrel','www','global','base','root',
  // structural/generic labels that are never subsystem names
  'example','examples','sample','samples','snippet','note','notes','detail','details',
  'result','results','output','input','data','value','values','item','items',
]);

function tokenize(text) {
  return normalizeProperNouns(text)
    .replace(/([a-z0-9])([A-Z])/g, '$1 $2')
    .replace(/[_/.\-:{}()[\],#"`'<>|=+*!?@$%^&~→←↑↓—–]+/g, ' ')
    .toLowerCase()
    .split(/\s+/)
    .map(t => t.trim())
    .filter(t => t.length > 1 && !/^\d+$/.test(t) && !FILE_EXTS.has(t));
}

function countMap(tokens) {
  const m = new Map();
  for (const t of tokens) m.set(t, (m.get(t) ?? 0) + 1);
  return m;
}

// ── RAKE keyword extraction ────────────────────────────────────────────────────

function rake(text) {
  if (!text) return [];
  const stopPattern = new RegExp(
    `\\b(${[...STOP].map(w => w.replace(/[-.*+?^${}()|[\]\\]/g, '\\$&')).join('|')})\\b|[^a-zA-Z0-9'\\-]+`,
    'gi'
  );
  const normalized = normalizeProperNouns(text).toLowerCase();
  const parts = normalized.split(stopPattern)
    .map(p => (p ?? '').trim())
    .filter(p => p && /[a-z]/.test(p));

  const candidates = [];
  for (const part of parts) {
    const words = part.split(/\s+/).filter(w => w.length > 1 && !/^\d+$/.test(w) && !NOISE.has(w) && !FILE_EXTS.has(w));
    if (words.length === 0) continue;
    for (let start = 0; start < words.length; start++) {
      for (let len = 1; len <= Math.min(4, words.length - start); len++) {
        candidates.push(words.slice(start, start + len));
      }
    }
  }

  if (candidates.length === 0) return [];

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

  const seen = new Set();
  const scored = [];
  for (const phrase of candidates) {
    const key = phrase.join(' ');
    if (seen.has(key)) continue;
    seen.add(key);
    if (phrase.every(w => STOP.has(w) || NOISE.has(w))) continue;
    // Reject phrases with any adjacent duplicate words ("extension extension") or all-same words
    if (phrase.length > 1 && (
      new Set(phrase).size === 1 ||
      phrase.some((w, i) => i > 0 && w === phrase[i - 1])
    )) continue;
    const score = phrase.reduce((sum, w) => sum + (wordScore.get(w) ?? 0), 0);
    scored.push({ phrase: key, words: phrase, score });
  }

  return scored.sort((a, b) => b.score - a.score);
}

// ── Cross-document co-presence ────────────────────────────────────────────────

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

function extractTargetRole(targetPath, targetTitle) {
  if (targetTitle) {
    const t = tokenize(targetTitle).filter(w => !STOP.has(w) && !NOISE.has(w));
    if (t.length > 0) return t.slice(0, 3).join(' ');
  }
  if (!targetPath) return 'target';
  const segments = targetPath.split('/').map(s => s.replace(/\.[^.]+$/, '').toLowerCase());
  for (let i = segments.length - 1; i >= 0; i--) {
    const seg = segments[i];
    if (!NOISE.has(seg) && !STOP.has(seg) && seg.length > 2 && !/^\d+$/.test(seg)) {
      return seg.replace(/[-_]/g, ' ');
    }
  }
  // All segments were noise; don't return a noise word — use 'target' as the sentinel
  return 'target';
}

// ── Source role extraction ─────────────────────────────────────────────────────

function extractSourceRole(headingChain, sourceTitle) {
  for (let i = headingChain.length - 1; i >= 0; i--) {
    const h = headingChain[i];
    const tokens = tokenize(h).filter(w => !STOP.has(w) && !NOISE.has(w));
    if (tokens.length > 0) return tokens.slice(0, 4).join(' ');
  }
  if (sourceTitle) {
    const tokens = tokenize(sourceTitle).filter(w => !STOP.has(w) && !NOISE.has(w));
    if (tokens.length > 0) return tokens.slice(0, 3).join(' ');
  }
  return 'documentation';
}

// ── Relationship type detection ────────────────────────────────────────────────

// Normalize two strings for tautology comparison: lowercase, collapse spaces/hyphens/underscores
function normCmp(s) { return s.toLowerCase().replace(/[-_\s]+/g, ''); }

const REL_TYPES = [
  {
    type: 'contract', threshold: 2,
    words: new Set(['schema','payload','shape','validates','parses','matches','expects','contract','format','structure','serializes','deserializes']),
    why: (core, obj, src, tgt) => {
      if (normCmp(obj) === normCmp(tgt) || normCmp(core) === normCmp(obj)) {
        return `${cap(core)} data contract in ${tgt}, as specified in the ${src} wiki section.`;
      }
      // Avoid "shape shape" — if obj already contains "shape", use "structure" as descriptor
      const shapeWord = /\bshape\b/i.test(obj) ? 'structure' : 'shape';
      return `${cap(core)} contract that synchronizes the ${obj} ${shapeWord} expected by the ${src} wiki section with what ${tgt} provides.`;
    },
  },
  {
    type: 'rule', threshold: 2,
    words: new Set(['validates','enforces','governs','policy','forbidden','allowed','permission','guard','boundary','invariant','constraint','rejects','denies']),
    why: (core, obj, src, tgt) => {
      if (normCmp(obj) === normCmp(tgt)) return `${cap(core)} enforcement rule in ${tgt}, as specified in the ${src} wiki section.`;
      return `${cap(core)} enforcement rule shared between the ${src} wiki section and the ${tgt} implementation.`;
    },
  },
  {
    type: 'flow', threshold: 3,
    words: new Set(['submits','routes','dispatches','pipeline','webhook','endpoint','handler','request','response','emits','propagates','triggers','subscribes']),
    why: (core, obj, src, tgt) => {
      if (normCmp(obj) === normCmp(tgt)) return `${cap(core)} flow in ${tgt}, as described in the ${src} wiki section.`;
      return `${cap(core)} flow that routes ${obj} as documented in the ${src} wiki section and implemented in ${tgt}.`;
    },
  },
  {
    type: 'config', threshold: 2,
    words: new Set(['config','configuration','settings','env','environment','wrangler','variable','flag','option','binding','secret','deploy']),
    why: (core, obj, src, tgt) => {
      if (normCmp(obj) === normCmp(tgt)) return `${cap(core)} configuration in ${tgt}, as specified in the ${src} wiki section.`;
      return `${cap(core)} configuration that the ${src} wiki section specifies and ${tgt} consumes.`;
    },
  },
  {
    // Default: wiki is a non-fiction reference. The why names what the section refers to
    // and where it lives — no subsystem or specification claim, just the referential link.
    type: 'sync', threshold: 0, words: new Set([]),
    why: (core, obj, src, tgt) => {
      if (normCmp(core) === normCmp(tgt)) {
        // core and target are the same concept — name the section without repeating the target
        return `${cap(core)} — covered by the ${src} wiki section.`;
      }
      if (normCmp(obj) === normCmp(tgt)) {
        return `${cap(core)} — the ${src} wiki section describes ${tgt}.`;
      }
      return `${cap(core)} — the ${src} wiki section describes ${obj} in ${tgt}.`;
    },
  },
];

function detectRelType(allTokens) {
  for (const rel of REL_TYPES.slice(0, -1)) {
    let score = 0;
    for (const t of allTokens) if (rel.words.has(t)) score++;
    if (score >= rel.threshold) return rel;
  }
  return REL_TYPES[REL_TYPES.length - 1];
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
    // allTokens includes surroundingText context which may bleed from nearby links;
    // weight it less than path/heading which are structural anchors of the page.
    for (const t of allTokens) if (ws.has(t)) score += 0.5;
    for (const t of pathTokens) if (ws.has(t)) score += 2;
    for (const t of headingTokens) if (ws.has(t)) score += 3;
    if (!best || score > best.score) best = { cat, score };
  }
  // Require at least one structural signal (path or heading) to assign a category
  return best && best.score >= 3 ? best.cat : null;
}

// ── Core phrase selection ──────────────────────────────────────────────────────

// Gerunds and past-tense verbs as sole core phrase are not subsystem names
const VERB_ONLY_RE = /^(adding|removing|updating|creating|building|extending|extend|defining|declaring|checking|handling|loading|saving|parsing|rendering|returning|using|getting|setting|having|making|calling|running|sending|showing|starting|stopping|enabling|disabling|changing|moving|wiring|mapping|reading|writing|marking|tracking|processing|generating|computing|resolving|detecting|scanning|enforcing|dispatching|routing|declared|defined|created|removed|updated|added|extended|checked|re|one|two|three|many|more)$/i;

function isWeakCorePhrase(phrase) {
  const words = phrase.trim().split(/\s+/);
  // Single-word gerund/verb → weak
  if (words.length === 1 && VERB_ONLY_RE.test(words[0])) return true;
  // Single char or two-char word → weak
  if (slugify(phrase).replace(/-/g, '').length < 3) return true;
  return false;
}

function selectCorePhrase(rakeResults, coPresent, linkText, targetPath, sourceTitleTokens) {
  const exclude = sourceTitleTokens;
  function titleDominated(phrase) {
    const words = phrase.split(/\s+/).filter(Boolean);
    if (words.length === 0) return true;
    return words.filter(w => exclude.has(w)).length / words.length > 0.5;
  }

  for (const { phrase } of rakeResults) {
    if (!titleDominated(phrase) && phrase.length > 2 && !isWeakCorePhrase(phrase)) return phrase;
  }
  if (coPresent.length > 0) {
    const top = coPresent.slice(0, 3).map(x => x.term).join(' ');
    if (!titleDominated(top) && top.length > 2 && !isWeakCorePhrase(top)) return top;
  }
  if (linkText) {
    const cleaned = tokenize(linkText)
      .filter(w => !STOP.has(w) && !NOISE.has(w))
      .filter((w, i, arr) => i === 0 || w !== arr[i - 1])  // remove adjacent duplicates
      .slice(0, 4).join(' ');
    if (cleaned && !titleDominated(cleaned) && !isWeakCorePhrase(cleaned)) return cleaned;
  }
  if (targetPath) {
    const seg = extractTargetRole(targetPath, null);
    if (seg && seg !== 'target') return seg;
  }
  // Last resort: accept any RAKE result even if weak (something > nothing)
  return rakeResults.find(r => !titleDominated(r.phrase) && r.phrase.length > 2)?.phrase
    ?? rakeResults[0]?.phrase ?? linkText ?? 'relationship';
}

// ── Prose why extraction ───────────────────────────────────────────────────────

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
      // also strip short parenthetical content like "( or )" "(`prod` or `staging`)"
      .map(c => c.replace(/\([^)]{0,50}\)/g, '').trim())
      .filter(c => c.length > 0)
      .sort((a, b) => b.length - a.length)[0];
    if (desc) {
      let tdesc = desc.replace(/[.,;: ]+$/, '').trim();
      const realChars = tdesc.replace(/[.,;:\s]/g, '').length;
      if (realChars < 15) return null;
      // Apply same basic quality checks as prose path
      if (/\b(at|by|in|on|from|to|with|and|or|as|is|was|were|the|of)\s*$/.test(tdesc)) return null;
      if (/^(and|or|but|nor|when|while|after|before|until|once)\b/i.test(tdesc)) return null;
      return cap(tdesc) + '.';
    }
    return null;
  }

  // Headings: no prose available
  if (raw.startsWith('#')) return null;

  // Build prose: strip markdown syntax, normalize special characters
  let prose = raw
    .replace(/`[^`\n]+`/g, ' __CODE__ ')   // replace backtick spans with placeholder
    .replace(/\[([^\[\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[\[([^\]|]+)(?:\|([^\]]*))?\]\]/g, (_, t, d) => d ?? t)
    .replace(/[→←↑↓]/g, ' ')               // strip arrow chars
    .replace(/—|–/g, ' ')                   // em/en dashes → space
    .replace(/L\d+(?:-L\d+)?/g, '')         // strip line number refs
    .replace(/^[#\-*0-9.> ]+/, '')          // strip list/heading prefixes
    .replace(/\*\*([^*]+)\*\*/g, '$1')
    .replace(/\*([^*]+)\*/g, '$1')
    .trim();

  // Clean up __CODE__ placeholder: remove it plus any orphaned punctuation it leaves
  prose = prose
    // "word, __CODE__, word" → "word, word"
    .replace(/,?\s*__CODE__\s*,?/g, ' ')
    // "by __CODE__" at end of clause → remove
    .replace(/\b(by|with|from|to|in|at|of|and|or|via)\s+__CODE__/gi, ' ')
    .replace(/__CODE__/g, ' ');

  prose = prose
    .replace(/\([^)]*\)/g, '')
    .replace(/\*+/g, '')
    .replace(/,\s*,+/g, ',')
    .replace(/\s+/g, ' ')
    .replace(/\s+([.,;:])/g, '$1')
    .replace(/^[,;:\s—–-]+/, '')
    .replace(/[;:,]\s*$/, '')                             // strip trailing ; : ,
    .replace(/\b(for|to|in|at|of|by|from|with|and|or)\s*$/, '')
    .trim();

  // Truncate to first sentence, but hard-cap at 140 chars first
  if (prose.length > 140) prose = prose.slice(0, 140).replace(/\s\S*$/, '');
  const sentEnd = prose.search(/[.!?]/);
  if (sentEnd !== -1) {
    prose = prose.slice(0, sentEnd + 1);
  } else if (prose && !prose.endsWith('.')) {
    prose += '.';
  }
  // Clean up colon-before-period and semicolon-before-period artifacts ("frame:." → "frame.")
  prose = prose.replace(/[;:,]([.!?])$/, '$1');

  // Reject prose starting with orphaned conjunction or temporal clause fragment
  if (/^(and|or|but|nor)\b/i.test(prose)) return null;
  // Reject "Both and", "Both ," — first part of two-part conjunction with subject stripped
  if (/^(both|either|neither)\s+(and|or|,)/i.test(prose)) return null;
  // Temporal/conditional openers that aren't subsystem descriptions
  if (/^(when|while|after|before|until|once|if|unless|since)\b/i.test(prose) && prose.split(/\s+/).length < 8) return null;

  // Fix headless predicates
  const HEADLESS = /^(is|are|was|were|applies|reserves|decides|builds|exports|stores|handles|manages|validates|parses|wraps|maps|tracks|owns|uses|caches|returns|emits|reads|writes|checks|runs|renders|sends|receives|creates|updates|deletes|fetches|loads|saves|generates|computes|resolves|detects|scans|enforces|processes|dispatches|routes|mounts|binds|wires|exposes|provides|accepts|listens|subscribes|publishes|registers|connects|wraps|extends|overrides|implements)\b/i;
  if (HEADLESS.test(prose)) {
    const label = link.originalText.replace(/[`*_[\]]/g, '').trim();
    if (label && !/^[A-Za-z0-9_\-/.]+\.[a-z]{1,5}$/.test(label)) {
      prose = cap(label) + ' ' + prose[0].toLowerCase() + prose.slice(1);
    }
  }

  // Reject: too short (< 20 real chars)
  const realContent = prose.replace(/[.,;:\s]/g, '');
  if (realContent.length < 20) return null;

  // Reject: just a filename or path
  if (/^[A-Za-z0-9_\-/.]+\.[a-z]{1,5}\.?$/.test(prose.trim())) return null;
  if (/^[A-Za-z0-9_-]+\/[A-Za-z0-9_./-]+\.?$/.test(prose.trim())) return null;

  // Reject: contains a path fragment (word/word anywhere in the string)
  if (/\b[a-z][a-z0-9_-]*\/[a-z][a-z0-9_-]/.test(prose)) return null;

  // Reject: bare function/identifier name (one or two CamelCase tokens with no predicate)
  if (/^[A-Z][a-z]+(?:[A-Z][a-z]+)*[.!?]$/.test(prose.trim())) return null;
  if (/^[A-Z][a-zA-Z]+ [A-Z][a-z]+(?:[A-Z][a-z]+)+[.!?]$/.test(prose.trim())) return null;

  // Reject: "X but" where X is a short first clause (subject was stripped, leaving "X but Y")
  if (/^\S[\w\s]{0,25}\bbut\b/i.test(prose) && prose.indexOf(' but ') < 30) return null;

  // Reject: "both and" with no items between — the conjuncts were backtick-stripped
  if (/\bboth\s+and\b/.test(prose)) return null;

  // Reject: "Identifier uses/is/returns/was." (subject + verb, no predicate)
  if (/^[A-Za-z]\S+\s+(uses|is|was|returns|extends|implements|wraps|exports)\.$/.test(prose.trim())) return null;

  // Reject prose starting with a leading slash (stripped code ref artifact)
  if (/^\//.test(prose)) return null;

  // Reject: ends with possessive genitive (sentence was cut off: "on the renderer's.")
  if (/\w+'s\s*[.!?]$/.test(prose)) return null;

  // Reject: ends with a trailing preposition, gerund, or common orphan
  if (/\b(at|by|in|on|from|to|with|and|or|as|is|was|were|the|of|into|via|requiring|containing|including|using|having|being)\s*[.!?]$/.test(prose)) return null;

  // Reject: trailing gerund phrase — sentence ending with "...ing."
  if (/\b\w+ing[.!?]$/.test(prose) && !/\b(thing|building|setting|something|everything|anything|nothing)\b/.test(prose)) {
    // only reject single-word trailing gerunds; compound phrases may be fine
    if (/\b\w+ing[.!?]$/.test(prose) && prose.split(/\s+/).length < 6) return null;
  }

  // Reject: heading-label pattern — short text ending with colon (bold section labels)
  if (/^[A-Za-z][^.!?]{0,60}:\s*\.?$/.test(prose.trim())) return null;

  // Ensure sentence starts with capital letter
  prose = prose.length > 0 ? prose[0].toUpperCase() + prose.slice(1) : prose;

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
    .filter(t => t.length > 1 && !FILE_EXTS.has(t))
    .join('-')
    .replace(/-+/g, '-')
    .replace(/^-|-$/g, '')
    || 'relationship';
}

function cap(s) { return s ? s[0].toUpperCase() + s.slice(1) : s; }

// ── Main mesh generation ───────────────────────────────────────────────────────

function generateMesh(link, sourceTitle, sourceContentSummary, targetMeta, root) {
  const sourceTitleTokens = new Set(tokenize(sourceTitle ?? ''));

  let targetSnippet = null;
  if (targetMeta.content) {
    const lines = targetMeta.content.split('\n');
    targetSnippet = lines.slice(Math.max(0, link.startLine - 1), Math.min(lines.length, link.endLine + 5)).join('\n');
  }

  const sourceCtx = [link.surroundingText, ...link.headingChain].join(' ');
  const targetCtx = [targetSnippet, targetMeta.title, targetMeta.summary].filter(Boolean).join(' ');

  const rakeResults = rake(link.surroundingText ?? '');
  const coPresent = coPresenceTerms(sourceCtx, targetCtx);

  // Strip bare line-range labels (L19-L22) — these are not subsystem names
  // Also deduplicate adjacent repeated words ("extension extension" → "extension")
  const linkText = link.originalText.replace(/[`*_[\]]/g, '').trim()
    .replace(/^L\d+(-L?\d+)?$/i, '')
    .replace(/\b(\w+)(\s+\1)+\b/gi, '$1');
  const corePhrase = selectCorePhrase(rakeResults, coPresent, linkText, link.path, sourceTitleTokens);

  const allTokens = tokenize(`${sourceCtx} ${targetCtx}`);
  const pathTokens = tokenize(`${relative(root, link.wikiFile).replace(/\\/g, '/')} ${link.path}`);
  const headingTokens = tokenize([sourceTitle, ...link.headingChain].join(' '));

  const relType = detectRelType(allTokens);
  const category = detectCategory(allTokens, pathTokens, headingTokens);

  const targetRole = extractTargetRole(link.path, targetMeta.title);
  const sourceRole = extractSourceRole(link.headingChain, sourceTitle);

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

  const proseWhy = extractProseWhy(link);

  // When corePhrase is a bare line-range (L19-L22), it's not a useful label for why/name
  const effectiveCore = /^L\d+(-L?\d+)?$/i.test(corePhrase.trim())
    ? (objectPhrase && normCmp(objectPhrase) !== normCmp(targetRole) ? objectPhrase : targetRole)
    : corePhrase;

  const why = proseWhy ?? templateWhy(relType, effectiveCore, objectPhrase, sourceRole, targetRole);

  // Name: wiki/<category>/<coreSlug>
  const coreSlug = (() => {
    const slug = slugify(effectiveCore);
    // If slug duplicates category, or is a line-number pattern, use targetRole
    if ((category && slug === category) || /^l\d+-l\d+$/i.test(slug) || slug === 'relationship') {
      return slugify(targetRole) || slug;
    }
    return slug;
  })();
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
console.log('# Generated by scripts/mesh-scaffold-v4.mjs');
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
