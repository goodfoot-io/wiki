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

export default postToolUseHook(
  { matcher: 'Edit|Write|MultiEdit|NotebookEdit', timeout: 30000 },
  (input, { logger }) => {
    const filePath = getFilePath(input);
    if (!filePath) return null;

    if (!isWikiFile(filePath, input.cwd)) return null;

    trackWikiFile(input.session_id, filePath);

    try {
      const result = spawnSync('wiki', ['check', filePath], {
        cwd: input.cwd,
        encoding: 'utf8',
        timeout: 25000,
        env: { ...process.env }
      });

      if (result.error) {
        logger.warn('wiki check execution error', { error: result.error.message });
        return null;
      }

      if (result.status === 0) return null;

      let output = [result.stdout, result.stderr].filter(Boolean).join('\n').trim();
      if (!output) return null;

      if (output.includes('mesh_uncovered')) {
        const filtered = output
          .split('\n')
          .filter((line) => !line.includes('mesh_uncovered'))
          .join('\n')
          .trim();
        try {
          const scaffoldResult = spawnSync('wiki', ['scaffold', filePath], {
            cwd: input.cwd,
            encoding: 'utf8',
            timeout: 25000,
            env: { ...process.env }
          });
          const scaffoldOutput =
            scaffoldResult.status === 0 && scaffoldResult.stdout ? scaffoldResult.stdout.trim() : '';
          output = [filtered, scaffoldOutput].filter(Boolean).join('\n\n');
        } catch {
          logger.warn('wiki scaffold unavailable; using check output as-is');
          output = filtered;
        }
      }

      if (!output) return null;

      logger.info('wiki check failed', { file: filePath, status: result.status });

      return postToolUseOutput({
        systemMessage: `<wiki>\n${output}\n</wiki>`,
        hookSpecificOutput: {
          additionalContext: `<wiki>\n${output}\n</wiki>`
        }
      });
    } catch {
      logger.warn('wiki command unavailable; skipping wiki check');
      return null;
    }
  }
);
