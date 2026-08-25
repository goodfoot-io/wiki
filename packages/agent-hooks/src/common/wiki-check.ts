import { type SpawnSyncReturns, spawnSync } from 'node:child_process';
import { closeSync, existsSync, openSync, readdirSync, readFileSync, readSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { isAbsolute, join, resolve } from 'node:path';

const FRONTMATTER_SCAN_BYTES = 4096;
const FRONTMATTER_SCAN_LINES = 30;
const DEFAULT_WIKI_CHECK_TIMEOUT_MS = 25000;

/**
 * Structural subset of the SDK loggers the platform adapters receive. The core
 * must not depend on any one SDK, so both the Claude and Codex loggers satisfy
 * this shape as-is.
 */
export interface WikiCheckLogger {
  info(message: string, context?: Record<string, unknown>): void;
  warn(message: string, context?: Record<string, unknown>): void;
}

/**
 * Read a bounded byte prefix from disk for the frontmatter head scan. A
 * frontmatter block must open at byte 0, so nothing past the window can change
 * the verdict; a block truncated mid-window fails closed exactly like a missing
 * close fence. The prefix is trimmed back to its last complete line so a torn
 * tail — including a split multibyte character at the window boundary — is
 * never scanned.
 */
export function readFrontmatterPrefix(absPath: string): string {
  const buf = Buffer.alloc(FRONTMATTER_SCAN_BYTES);
  const fd = openSync(absPath, 'r');
  try {
    const bytesRead = readSync(fd, buf, 0, FRONTMATTER_SCAN_BYTES, 0);
    let head = buf.toString('utf-8', 0, bytesRead);
    if (bytesRead === FRONTMATTER_SCAN_BYTES) {
      const lastNewline = head.lastIndexOf('\n');
      if (lastNewline !== -1) head = head.slice(0, lastNewline);
    }
    return head;
  } finally {
    closeSync(fd);
  }
}

/**
 * Returns true if the file is a wiki member, determined exclusively by YAML
 * frontmatter: both `title` and `summary` must be present and non-empty.
 * Non-.md files are never wiki members.
 */
export function isWikiFile(filePath: string, cwd: string): boolean {
  if (!filePath.endsWith('.md')) return false;

  const absPath = isAbsolute(filePath) ? filePath : resolve(cwd, filePath);
  if (!existsSync(absPath)) return false;

  // Read only the first 30 lines to locate frontmatter efficiently: scan a
  // bounded disk prefix instead of materializing the whole file.
  const head = readFrontmatterPrefix(absPath).split('\n').slice(0, FRONTMATTER_SCAN_LINES);

  if (head[0]?.trim() !== '---') return false;

  const closeIdx = head.slice(1).findIndex((l) => l.trim() === '---');
  if (closeIdx === -1) return false;

  const fmLines = head.slice(1, closeIdx + 1);
  let title = '';
  let summary = '';
  for (const line of fmLines) {
    const titleMatch = line.match(/^title\s*:\s*(.+)$/);
    if (titleMatch) title = titleMatch[1].trim().replace(/^['"]|['"]$/g, '');
    const summaryMatch = line.match(/^summary\s*:\s*(.+)$/);
    if (summaryMatch) summary = summaryMatch[1].trim().replace(/^['"]|['"]$/g, '');
  }

  return title.length > 0 && summary.length > 0;
}

/** Per-session scratch file listing every wiki file the session has touched. */
export function sessionTrackingFile(sessionId: string): string {
  return join(tmpdir(), `wiki-check-${sessionId}.txt`);
}

export function trackWikiFile(sessionId: string, filePath: string): void {
  const trackingFile = sessionTrackingFile(sessionId);
  let existing: string[] = [];
  if (existsSync(trackingFile)) {
    existing = readFileSync(trackingFile, 'utf-8')
      .split('\n')
      .map((l) => l.trim())
      .filter(Boolean);
  }
  if (!existing.includes(filePath)) {
    existing.push(filePath);
    writeFileSync(trackingFile, `${existing.join('\n')}\n`, 'utf-8');
  }
}

export const WIKI_EXECUTABLE = process.platform === 'win32' ? 'wiki.exe' : 'wiki';

/** Compare dotted numeric versions (e.g. `0.5.74`); returns a<b => negative. */
export function compareSemver(a: string, b: string): number {
  const pa = a.split('.').map((n) => Number.parseInt(n, 10));
  const pb = b.split('.').map((n) => Number.parseInt(n, 10));
  for (let i = 0; i < Math.max(pa.length, pb.length); i += 1) {
    const da = pa[i] ?? 0;
    const db = pb[i] ?? 0;
    if (Number.isNaN(da) || Number.isNaN(db)) return a.localeCompare(b);
    if (da !== db) return da - db;
  }
  return 0;
}

/** Candidate VS Code `globalStorage` roots across editions and platforms. */
export function vscodeGlobalStorageRoots(): string[] {
  const home = homedir();
  const roots = [
    join(home, '.vscode-server', 'data', 'User', 'globalStorage'),
    join(home, '.vscode-server-insiders', 'data', 'User', 'globalStorage'),
    join(home, '.config', 'Code', 'User', 'globalStorage'),
    join(home, '.config', 'Code - Insiders', 'User', 'globalStorage'),
    join(home, 'Library', 'Application Support', 'Code', 'User', 'globalStorage'),
    join(home, 'Library', 'Application Support', 'Code - Insiders', 'User', 'globalStorage')
  ];
  const appData = process.env.APPDATA;
  if (appData) {
    roots.push(join(appData, 'Code', 'User', 'globalStorage'));
    roots.push(join(appData, 'Code - Insiders', 'User', 'globalStorage'));
  }
  return roots;
}

/**
 * Locate the `wiki` binary the VS Code extension installs under its
 * `globalStorage` (`<root>/goodfoot.wiki-extension/bin/<version>/<target>/wiki`).
 * The extension only injects this onto the *integrated terminal* PATH, so a
 * hook subprocess never inherits it — we must find it on disk. Newest version
 * wins. Returns null when no managed binary is present.
 */
export function findManagedWikiBinary(): string | null {
  for (const root of vscodeGlobalStorageRoots()) {
    const binRoot = join(root, 'goodfoot.wiki-extension', 'bin');
    if (!existsSync(binRoot)) continue;

    let versions: string[];
    try {
      versions = readdirSync(binRoot);
    } catch {
      continue;
    }
    versions.sort((a, b) => compareSemver(b, a)); // newest first

    for (const version of versions) {
      const versionDir = join(binRoot, version);
      let targets: string[];
      try {
        targets = readdirSync(versionDir);
      } catch {
        continue;
      }
      for (const target of targets) {
        const candidate = join(versionDir, target, WIKI_EXECUTABLE);
        if (existsSync(candidate)) return candidate;
      }
    }
  }
  return null;
}

/**
 * Resolve an absolute path to the `wiki` binary, tolerant of the fact that an
 * agent-hook subprocess does not inherit the extension-augmented terminal PATH.
 * Resolution order: `WIKI_BIN` override, then PATH, then the extension's managed
 * binary. Falls back to the bare name so the caller's spawn surfaces a clear
 * ENOENT when nothing is found.
 */
export function resolveWikiBinary(logger?: WikiCheckLogger): string {
  const override = process.env.WIKI_BIN;
  if (override && existsSync(override)) return override;

  const whichCmd = process.platform === 'win32' ? 'where' : 'which';
  const onPath = spawnSync(whichCmd, [WIKI_EXECUTABLE], { encoding: 'utf8' });
  if (onPath.status === 0 && onPath.stdout) {
    const first = onPath.stdout
      .split(/\r?\n/)
      .map((l) => l.trim())
      .filter(Boolean)[0];
    if (first && existsSync(first)) return first;
  }

  const managed = findManagedWikiBinary();
  if (managed) {
    logger?.info('resolved wiki binary from VS Code globalStorage', { path: managed });
    return managed;
  }

  return WIKI_EXECUTABLE;
}

/** Outcome of one `wiki check --fix` invocation. */
export type WikiCheckStatus = 'clean' | 'residual' | 'unavailable';

export interface WikiCheckResult {
  status: WikiCheckStatus;
  /**
   * Combined stdout/stderr: residual diagnostics when `status` is `'residual'`,
   * the launch-failure detail when `'unavailable'`. Absent when there is
   * nothing to surface.
   */
  output?: string;
}

export interface WikiCheckOptions {
  /** Absolute or PATH-resolvable path to the `wiki` binary to spawn. */
  binary: string;
  /** Spawn timeout in milliseconds; defaults to {@link DEFAULT_WIKI_CHECK_TIMEOUT_MS}. */
  timeoutMs?: number;
  /** Working directory for the spawn; defaults to the current process cwd. */
  cwd?: string;
}

/**
 * Run `wiki check --fix <file>` once and classify the outcome:
 * - `'clean'` — exit 0; the page was validated (and auto-fixed in place).
 * - `'residual'` — non-zero exit with collected diagnostics the agent must resolve.
 * - `'unavailable'` — the binary could not be launched at all (or the spawn threw).
 *
 * Never throws: every failure mode lands in the result so adapters can decide
 * their own fail-open/fail-closed surfacing.
 */
export function runWikiCheck(filePath: string, options: WikiCheckOptions): WikiCheckResult {
  let result: SpawnSyncReturns<string>;
  try {
    result = spawnSync(options.binary, ['check', '--fix', filePath], {
      cwd: options.cwd,
      encoding: 'utf8',
      timeout: options.timeoutMs ?? DEFAULT_WIKI_CHECK_TIMEOUT_MS,
      env: { ...process.env }
    });
  } catch (err) {
    return { status: 'unavailable', output: err instanceof Error ? err.message : String(err) };
  }

  if (isLaunchFailure(result)) {
    return { status: 'unavailable', output: result.error?.message ?? 'spawn failed' };
  }

  if (result.status !== 0) {
    const output = [result.stdout, result.stderr].filter(Boolean).join('\n').trim();
    return output ? { status: 'residual', output } : { status: 'residual' };
  }

  return { status: 'clean' };
}

/** True when a spawn failed because the binary itself could not be launched. */
function isLaunchFailure(result: SpawnSyncReturns<string>): boolean {
  return result.error != null;
}
