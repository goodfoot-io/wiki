import { getFilePath, postToolUseHook, postToolUseOutput } from '@goodfoot/agent-hooks/claude-code';
import {
  isWikiFile,
  resolveWikiBinary,
  runWikiCheck,
  wikiContextBlock,
  wikiUnavailableBlock
} from '../common/wiki-check.js';

/** The hook's own spawn timeout is separately bounded below the registration budget. */
const WIKI_CHECK_TIMEOUT_MS = 25000;

/**
 * Fail-closed surfacing: when the `wiki` binary cannot be launched, the page's
 * links and line-range drift went unvalidated. The hook fires after the write, so
 * it cannot block — but it must make the gap loud rather than passing silently.
 */
function wikiUnavailableOutput(filePath: string, wikiBin: string, detail: string) {
  const block = wikiUnavailableBlock(filePath, wikiBin, detail);
  return postToolUseOutput({
    systemMessage: block,
    hookSpecificOutput: { additionalContext: block }
  });
}

export default postToolUseHook({ matcher: 'Edit|Write|NotebookEdit', timeout: 60000 }, (input, { logger }) => {
  const filePath = getFilePath(input);
  if (!filePath) return null;

  if (!isWikiFile(filePath, input.cwd)) return null;

  const wikiBin = resolveWikiBinary(logger);

  // Single invocation: auto-fix line-range drift and frontmatter; a non-zero
  // exit means residual, unfixable wiki conditions the agent must resolve.
  const result = runWikiCheck(filePath, { binary: wikiBin, timeoutMs: WIKI_CHECK_TIMEOUT_MS, cwd: input.cwd });

  if (result.status === 'unavailable') {
    const detail = result.output ?? 'spawn failed';
    logger.warn('wiki check execution error', { error: detail, wikiBin });
    return wikiUnavailableOutput(filePath, wikiBin, detail);
  }

  if (result.status === 'residual' && result.output) {
    logger.info('wiki check failed', { file: filePath });
    const block = wikiContextBlock(result.output);
    return postToolUseOutput({
      systemMessage: block,
      hookSpecificOutput: { additionalContext: block }
    });
  }

  return null;
});
