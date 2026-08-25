import { type HookContext, type PostToolUseInput, postToolUseHook, postToolUseOutput } from '@goodfoot/codex-hooks';
import {
  extractPatchedFilePaths,
  isWikiFile,
  resolveWikiBinary,
  runWikiCheck,
  wikiContextBlock,
  wikiUnavailableBlock
} from '../common/wiki-check.js';

const WIKI_CHECK_TIMEOUT_MS = 25000;

/** Codex tool names whose inputs may rewrite wiki files. */
export const WIKI_POST_MATCHER = 'apply_patch|exec_command|exec|shell|local_shell';

/** Narrow the SDK's unknown apply_patch input to its patch-text command. */
export function narrowPatchText(toolInput: unknown): string | null {
  if (
    toolInput !== null &&
    typeof toolInput !== 'undefined' &&
    typeof toolInput === 'object' &&
    'command' in toolInput
  ) {
    const command = (toolInput as { command: unknown }).command;
    if (typeof command === 'string') return command;
  }
  return null;
}

export function createHandler() {
  return async (input: PostToolUseInput, { logger }: HookContext) => {
    const patchText = narrowPatchText(input.tool_input);
    if (patchText === null) return undefined;

    const filePaths = extractPatchedFilePaths(patchText);
    if (filePaths.length === 0) return undefined;

    const wikiBin = resolveWikiBinary(logger);

    // Single pass over every touched wiki member: --fix auto-repairs drift in
    // place; non-zero exits mean residual conditions the agent must resolve.
    const sections: string[] = [];
    let unavailableDetail: string | null = null;
    for (const filePath of filePaths) {
      if (!isWikiFile(filePath, input.cwd)) continue;

      const result = runWikiCheck(filePath, { binary: wikiBin, timeoutMs: WIKI_CHECK_TIMEOUT_MS, cwd: input.cwd });
      if (result.status === 'unavailable') {
        unavailableDetail ??= result.output ?? 'spawn failed';
        continue;
      }
      if (result.status === 'residual' && result.output) sections.push(result.output);
    }

    if (unavailableDetail !== null) {
      logger.warn('wiki check execution error', { error: unavailableDetail, wikiBin });
      return postToolUseOutput({
        additionalContext: wikiUnavailableBlock(filePaths[0], wikiBin, unavailableDetail)
      });
    }

    if (sections.length === 0) return undefined;
    return postToolUseOutput({ additionalContext: wikiContextBlock(sections.join('\n\n')) });
  };
}

// The codex-hooks CLI extracts manifest metadata via AST and reads only
// inline string literals for `matcher`; referencing WIKI_POST_MATCHER here
// would silently drop the field. hooks-shape.test.ts pins the emitted form.
export default postToolUseHook(
  { matcher: 'apply_patch|exec_command|exec|shell|local_shell', timeout: 60000 },
  createHandler()
);
