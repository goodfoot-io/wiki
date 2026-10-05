import {
  type HookContext,
  type PostToolUseInput,
  postToolUseHook,
  postToolUseOutput
} from '@goodfoot/agent-hooks/codex';
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
  if (typeof toolInput !== 'object' || toolInput === null || !('command' in toolInput)) return null;
  const { command } = toolInput;
  return typeof command === 'string' ? command : null;
}

export function createHandler() {
  return async (input: PostToolUseInput, { logger }: HookContext) => {
    const patchText = narrowPatchText(input.tool_input);
    if (patchText === null) return undefined;

    // The SDK types `cwd` as a string but performs no runtime validation of
    // the payload; treat it as untrusted before resolving paths against it.
    const cwd: unknown = input.cwd;
    if (typeof cwd !== 'string' || cwd.length === 0) {
      logger.warn('malformed PostToolUse payload — cwd missing or not a non-empty string; wiki check skipped');
      return undefined;
    }

    const filePaths = extractPatchedFilePaths(patchText);
    if (filePaths.length === 0) return undefined;

    const wikiBin = resolveWikiBinary(logger);

    // Single pass over every touched wiki member: --fix auto-repairs drift in
    // place; non-zero exits mean residual conditions the agent must resolve.
    const sections: string[] = [];
    let unavailable: { filePath: string; detail: string } | null = null;
    for (const filePath of filePaths) {
      if (!isWikiFile(filePath, cwd)) continue;

      const result = runWikiCheck(filePath, { binary: wikiBin, timeoutMs: WIKI_CHECK_TIMEOUT_MS, cwd });
      if (result.status === 'unavailable') {
        unavailable ??= { filePath, detail: result.output ?? 'spawn failed' };
        continue;
      }
      if (result.status === 'residual' && result.output) sections.push(result.output);
    }

    if (unavailable !== null) {
      logger.warn('wiki check execution error', { error: unavailable.detail, wikiBin });
      return postToolUseOutput({
        additionalContext: wikiUnavailableBlock(unavailable.filePath, wikiBin, unavailable.detail)
      });
    }

    if (sections.length === 0) return undefined;
    return postToolUseOutput({ additionalContext: wikiContextBlock(sections.join('\n\n')) });
  };
}

// The unified agent-hooks CLI extracts manifest metadata via AST and reads
// only inline string literals for `matcher`; referencing WIKI_POST_MATCHER
// here would silently drop the field. hooks-shape.test.ts pins the emitted form.
export default postToolUseHook(
  { matcher: 'apply_patch|exec_command|exec|shell|local_shell', timeout: 60000 },
  createHandler()
);
