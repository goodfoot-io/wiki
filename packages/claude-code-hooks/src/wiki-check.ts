import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { getFilePath, postToolUseHook, postToolUseOutput } from '@goodfoot/claude-code-hooks';

function isWikiFile(filePath: string, cwd: string): boolean {
  if (filePath.endsWith('.wiki.md')) return true;

  const absPath = isAbsolute(filePath) ? filePath : resolve(cwd, filePath);
  let dir = dirname(absPath);
  while (true) {
    if (existsSync(join(dir, 'wiki.toml'))) return true;
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return false;
}

export default postToolUseHook(
  { matcher: 'Edit|Write|MultiEdit|NotebookEdit', timeout: 30000 },
  (input, { logger }) => {
    const filePath = getFilePath(input);
    if (!filePath) return null;

    if (!isWikiFile(filePath, input.cwd)) return null;

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
        try {
          const scaffoldResult = spawnSync('wiki', ['scaffold', filePath], {
            cwd: input.cwd,
            encoding: 'utf8',
            timeout: 25000,
            env: { ...process.env }
          });
          if (scaffoldResult.status === 0 && scaffoldResult.stdout) {
            output = [output, scaffoldResult.stdout.trim()].filter(Boolean).join('\n\n');
          }
        } catch {
          logger.warn('wiki scaffold unavailable; using check output as-is');
        }
      }

      logger.info('wiki check failed', { file: filePath, status: result.status });

      return postToolUseOutput({
        continue: false,
        stopReason: output,
        hookSpecificOutput: {
          additionalContext: output
        }
      });
    } catch {
      logger.warn('wiki command unavailable; skipping wiki check');
      return null;
    }
  }
);
