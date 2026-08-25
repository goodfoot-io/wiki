import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { isWikiFile } from '../src/common/wiki-check.js';

const IO_BUDGET_BYTES = 64 * 1024;
const BIG_FILE_BYTES = 8 * 1024 * 1024;

let fixtureDir: string | undefined;

function makeFile(name: string, content: string): string {
  if (!fixtureDir) fixtureDir = mkdirSync(join(tmpdir(), `is-wiki-file-`), { recursive: true });
  const path = join(fixtureDir, name);
  writeFileSync(path, content, 'utf-8');
  return path;
}

afterEach(() => {
  if (fixtureDir) {
    rmSync(fixtureDir, { recursive: true, force: true });
    fixtureDir = undefined;
  }
});

describe('isWikiFile decision contract', () => {
  it('accepts a wiki member whose frontmatter closes near the 30-line boundary', () => {
    const lines = ['---', 'title: Boundary', 'summary: Closes at line 30'];
    while (lines.length < 29) lines.push('filler');
    lines.push('---', 'body');
    expect(isWikiFile(makeFile('boundary-at-30.md', lines.join('\n')), process.cwd())).toBe(true);
  });

  it('rejects a file whose frontmatter never closes within 30 lines', () => {
    const lines = ['---', 'title: Unclosed', 'summary: Fence drifts away'];
    while (lines.length < 40) lines.push('filler');
    lines.push('---', 'body');
    expect(isWikiFile(makeFile('unclosed.md', lines.join('\n')), process.cwd())).toBe(false);
  });

  it('rejects frontmatter missing title or summary', () => {
    expect(isWikiFile(makeFile('no-summary.md', '---\ntitle: Only Title\n---\nbody'), process.cwd())).toBe(false);
  });

  it('rejects files without an opening fence', () => {
    expect(isWikiFile(makeFile('no-fence.md', '# Just markdown\n\nplain body'), process.cwd())).toBe(false);
  });

  it('resolves relative paths against cwd', () => {
    const abs = makeFile('relative.md', '---\ntitle: Relative\nsummary: Resolved via cwd\n---\n');
    const dir = abs.slice(0, abs.lastIndexOf('/'));
    expect(isWikiFile('relative.md', dir)).toBe(true);
  });

  it('rejects frontmatter blocks that extend beyond the bounded read window', () => {
    // The closing fence sits ~40KB in — inside the first 30 lines but far past
    // any sane prefix window. Per the card contract, a frontmatter block longer
    // than the scanned prefix cannot be a valid member: fail closed.
    const lines = ['---', 'title: Oversized', 'summary: Block exceeds the window'];
    while (lines.length < 28) lines.push(`x${'p'.repeat(2048)}`);
    lines.push('---', 'body');
    expect(isWikiFile(makeFile('oversized-fm.md', lines.join('\n')), process.cwd())).toBe(false);
  });
});

const procIoReadable = (() => {
  try {
    return /^\s*rchar:\s*\d+/m.test(readFileSync('/proc/self/io', 'utf-8'));
  } catch {
    return false;
  }
})();

describe.skipIf(!procIoReadable)('isWikiFile bounded read', () => {
  function readRchar(): number {
    const match = readFileSync('/proc/self/io', 'utf-8').match(/^rchar:\s*(\d+)$/m);
    if (!match) throw new Error('rchar counter unavailable in /proc/self/io');
    return Number.parseInt(match[1], 10);
  }

  it('consumes bounded read bandwidth regardless of file size', () => {
    const big = makeFile('big-non-wiki.md', `# Not frontmatter\n\n${'x'.repeat(BIG_FILE_BYTES)}\n`);

    // Settle lazy fs-module init outside the measurement window.
    expect(isWikiFile(makeFile('warmup.md', 'warm'), process.cwd())).toBe(false);

    const before = readRchar();
    const result = isWikiFile(big, process.cwd());
    const after = readRchar();

    expect(result).toBe(false);
    expect(after - before).toBeLessThan(IO_BUDGET_BYTES);
  });

  it('consumes bounded bandwidth even when the file is a wiki member', () => {
    const lines = ['---', 'title: Big Member', 'summary: Valid but huge'];
    while (lines.length < 25) lines.push('filler');
    lines.push('---', `${'y'.repeat(BIG_FILE_BYTES)}`);
    const big = makeFile('big-wiki.md', lines.join('\n'));

    expect(isWikiFile(makeFile('warmup-member.md', 'warm'), process.cwd())).toBe(false);

    const before = readRchar();
    const result = isWikiFile(big, process.cwd());
    const after = readRchar();

    expect(result).toBe(true);
    expect(after - before).toBeLessThan(IO_BUDGET_BYTES);
  });
});
