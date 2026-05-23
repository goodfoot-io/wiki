import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { isAbsolute, join, resolve } from 'node:path';
import { getFilePath, postToolUseHook, postToolUseOutput } from '@goodfoot/claude-code-hooks';

/**
 * Returns true if the file is a wiki member, determined exclusively by YAML
 * frontmatter: both `title` and `summary` must be present and non-empty.
 * Non-.md files are never wiki members.
 */
function isWikiFile(filePath: string, cwd: string): boolean {
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

export default postToolUseHook({ matcher: 'Edit|Write|NotebookEdit', timeout: 60000 }, (input, { logger }) => {
  const filePath = getFilePath(input);
  if (!filePath) return null;

  if (!isWikiFile(filePath, input.cwd)) return null;

  trackWikiFile(input.session_id, filePath);

  // Sections accumulated across both phases, surfaced to the agent together.
  const sections: string[] = [];

  // ── Phase 1: auto-fix links/anchors/frontmatter (no mesh — phase 2 owns it).
  // --fix rewrites in place; a non-zero exit means residual, unfixable wiki
  // conditions the agent must resolve by hand.
  try {
    const result = spawnSync('wiki', ['check', '--fix', '--no-mesh', filePath], {
      cwd: input.cwd,
      encoding: 'utf8',
      timeout: 25000,
      env: { ...process.env }
    });

    if (result.error) {
      // `wiki` itself is missing or failed to launch — nothing else will work.
      logger.warn('wiki check execution error', { error: result.error.message });
      return null;
    }

    if (result.status !== 0) {
      const output = [result.stdout, result.stderr].filter(Boolean).join('\n').trim();
      if (output) {
        logger.info('wiki check failed', { file: filePath, status: result.status });
        sections.push(output);
      }
    }
  } catch {
    logger.warn('wiki command unavailable; skipping wiki check');
    return null;
  }

  // ── Phase 2: create git-mesh coverage for the page's fragment links.
  // --print-applied routes created/extended mesh paths to stdout and
  // advisories (dropped meshes, missing targets) to stderr. A non-zero exit
  // is a hard failure (git-mesh missing, or a `git mesh add` failure) and is
  // surfaced to the agent so mesh coverage gaps never pass silently.
  try {
    const result = spawnSync('wiki', ['scaffold', '--print-applied', filePath], {
      cwd: input.cwd,
      encoding: 'utf8',
      timeout: 25000,
      env: { ...process.env }
    });

    if (result.error) {
      logger.warn('wiki scaffold execution error', { error: result.error.message });
    } else {
      const applied = (result.stdout ?? '').trim();
      const advisories = (result.stderr ?? '').trim();

      if (result.status !== 0) {
        logger.info('wiki scaffold failed', { file: filePath, status: result.status });
        const detail = [advisories, applied].filter(Boolean).join('\n').trim();
        sections.push(`mesh scaffold failed (exit ${result.status}); fragment links may lack coverage:\n${detail}`);
      } else {
        if (applied) logger.info('wiki scaffold created meshes', { file: filePath, applied });
        if (advisories) {
          sections.push(`mesh scaffold advisories (links left uncovered):\n${advisories}`);
        }
      }
    }
  } catch {
    logger.warn('wiki scaffold unavailable; skipping mesh coverage');
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
