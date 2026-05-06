import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { stopHook, stopOutput } from '@goodfoot/claude-code-hooks';

function sessionTrackingFile(sessionId: string): string {
  return join(tmpdir(), `wiki-check-${sessionId}.txt`);
}

function readTrackedFiles(sessionId: string): string[] {
  const trackingFile = sessionTrackingFile(sessionId);
  if (!existsSync(trackingFile)) return [];
  return readFileSync(trackingFile, 'utf-8')
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);
}

function checkWikiFile(
  filePath: string,
  cwd: string,
  logger: { warn: (msg: string, data?: Record<string, unknown>) => void }
): { ok: boolean; output: string } {
  let result: ReturnType<typeof spawnSync>;
  try {
    result = spawnSync('wiki', ['check', filePath], {
      cwd,
      encoding: 'utf8',
      timeout: 25000,
      env: { ...process.env }
    });
  } catch (err) {
    logger.warn('wiki check spawn failed; blocking stop to avoid silent bypass', {
      file: filePath,
      error: String(err)
    });
    return { ok: false, output: 'wiki check infrastructure unavailable: unable to spawn wiki CLI' };
  }

  if (result.error) {
    logger.warn('wiki check infrastructure error; blocking stop to avoid silent bypass', {
      file: filePath,
      error: result.error.message
    });
    return { ok: false, output: `wiki check infrastructure unavailable: ${result.error.message}` };
  }

  if (result.status === 0) return { ok: true, output: '' };

  let output = [result.stdout, result.stderr].filter(Boolean).join('\n').trim();
  if (!output) return { ok: true, output: '' };

  if (output.includes('mesh_uncovered')) {
    const filtered = output
      .split('\n')
      .filter((line) => !line.includes('mesh_uncovered'))
      .join('\n')
      .trim();
    try {
      const scaffoldResult = spawnSync('wiki', ['scaffold', filePath], {
        cwd,
        encoding: 'utf8',
        timeout: 25000,
        env: { ...process.env }
      });
      const scaffoldOutput = scaffoldResult.status === 0 && scaffoldResult.stdout ? scaffoldResult.stdout.trim() : '';
      output = [filtered, scaffoldOutput].filter(Boolean).join('\n\n');
    } catch {
      output = filtered;
    }
  }

  if (!output) return { ok: true, output: '' };
  return { ok: false, output };
}

export default stopHook({}, (input, { logger }) => {
  const tracked = readTrackedFiles(input.session_id);
  if (tracked.length === 0) return null;

  const cwd = input.cwd;
  const existing = tracked.filter((f) => existsSync(f));
  if (existing.length === 0) return null;

  const failures: { file: string; output: string }[] = [];
  for (const filePath of existing) {
    const result = checkWikiFile(filePath, cwd, logger);
    if (!result.ok) {
      failures.push({ file: filePath, output: result.output });
    }
  }

  if (failures.length === 0) return null;

  logger.info('wiki check failures blocking stop', {
    count: failures.length,
    files: failures.map((f) => f.file)
  });

  const body = failures.map((f) => `<wiki>\n${f.output}\n</wiki>`).join('\n\n');

  const reason = [
    `${failures.length} edited wiki file(s) are invalid and must be updated before the turn can end:`,
    '',
    body,
    '',
    'Load the wiki:wiki skill for guidance on resolving these failures, then fix the files.'
  ].join('\n');

  return stopOutput({
    decision: 'block',
    reason,
    systemMessage: reason
  });
});
