import { type SpawnSyncReturns, spawnSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { isAbsolute, join, resolve } from 'node:path';
import { getFilePath, type Logger, postToolUseHook, postToolUseOutput } from '@goodfoot/claude-code-hooks';

/**
 * Returns true if the file is a wiki member, determined exclusively by YAML
 * frontmatter: both `title` and `summary` must be present and non-empty.
 * Non-.md files are never wiki members.
 */
export function isWikiFile(filePath: string, cwd: string): boolean {
  if (!filePath.endsWith('.md')) return false;

  const absPath = isAbsolute(filePath) ? filePath : resolve(cwd, filePath);
  if (!existsSync(absPath)) return false;

  // Read only the first 30 lines to locate frontmatter efficiently.
  const content = readFileSync(absPath, 'utf-8');
  const lines = content.split('\n');
  const head = lines.slice(0, 30);

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

function sessionTrackingFile(sessionId: string): string {
  return join(tmpdir(), `wiki-check-${sessionId}.txt`);
}

function trackWikiFile(sessionId: string, filePath: string): void {
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

const WIKI_EXECUTABLE = process.platform === 'win32' ? 'wiki.exe' : 'wiki';

/** Compare dotted numeric versions (e.g. `0.5.74`); returns a<b => negative. */
function compareSemver(a: string, b: string): number {
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
function vscodeGlobalStorageRoots(): string[] {
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
function findManagedWikiBinary(): string | null {
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
 * Resolve an absolute path to the `wiki` binary, tolerant of the fact that a
 * Claude Code hook subprocess does not inherit the extension-augmented terminal
 * PATH. Resolution order: `WIKI_BIN` override, then PATH, then the extension's
 * managed binary. Falls back to the bare name so the caller's spawn surfaces a
 * clear ENOENT when nothing is found.
 */
export function resolveWikiBinary(logger: Logger): string {
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
    logger.info('resolved wiki binary from VS Code globalStorage', { path: managed });
    return managed;
  }

  return WIKI_EXECUTABLE;
}

/** True when a spawn failed because the binary itself could not be launched. */
function isLaunchFailure(result: SpawnSyncReturns<string>): boolean {
  return result.error != null;
}

/**
 * Fail-closed surfacing: when the `wiki` binary cannot be launched, the page's
 * links and line-range drift went unvalidated. The hook fires after the write, so
 * it cannot block — but it must make the gap loud rather than passing silently.
 */
function wikiUnavailableOutput(filePath: string, wikiBin: string, detail: string) {
  const message =
    `wiki validation was SKIPPED — the \`wiki\` binary could not be launched (${detail}).\n` +
    `Resolved binary: ${wikiBin}\n` +
    `Fragment links and line-range drift for ${filePath} were NOT validated.\n` +
    'Install the wiki CLI on PATH, or set WIKI_BIN to its absolute path, then re-save the file.';
  const block = `<wiki>\n${message}\n</wiki>`;
  return postToolUseOutput({
    systemMessage: block,
    hookSpecificOutput: { additionalContext: block }
  });
}

export default postToolUseHook({ matcher: 'Edit|Write|NotebookEdit', timeout: 60000 }, (input, { logger }) => {
  const filePath = getFilePath(input);
  if (!filePath) return null;

  if (!isWikiFile(filePath, input.cwd)) return null;

  trackWikiFile(input.session_id, filePath);

  const wikiBin = resolveWikiBinary(logger);

  // ── Single invocation: auto-fix line-range drift and frontmatter.
  // --fix relocates drifted links and initializes links-reviewed in place;
  // a non-zero exit means residual, unfixable wiki conditions the agent must
  // resolve by hand.
  const sections: string[] = [];

  try {
    const result = spawnSync(wikiBin, ['check', '--fix', filePath], {
      cwd: input.cwd,
      encoding: 'utf8',
      timeout: 25000,
      env: { ...process.env }
    });

    if (isLaunchFailure(result)) {
      // The binary is missing or failed to launch. The page is now unvalidated —
      // surface it instead of failing open.
      const detail = result.error?.message ?? 'spawn failed';
      logger.warn('wiki check execution error', { error: detail, wikiBin });
      return wikiUnavailableOutput(filePath, wikiBin, detail);
    }

    if (result.status !== 0) {
      const output = [result.stdout, result.stderr].filter(Boolean).join('\n').trim();
      if (output) {
        logger.info('wiki check failed', { file: filePath, status: result.status });
        sections.push(output);
      }
    }
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err);
    logger.warn('wiki check threw', { error: detail, wikiBin });
    return wikiUnavailableOutput(filePath, wikiBin, detail);
  }

  if (sections.length === 0) return null;

  const output = sections.join('\n\n');
  return postToolUseOutput({
    systemMessage: `<wiki>\n${output}\n</wiki>`,
    hookSpecificOutput: {
      additionalContext: `<wiki>\n${output}\n</wiki>`
    }
  });
});
