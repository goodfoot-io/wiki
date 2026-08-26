/**
 * OpenCode plugin entry — wires the shared wiki-check core into the host's
 * `Hooks` shape. OpenCode loads this module in-process via file:// and calls
 * the default export's `server` once at startup; the returned `tool.execute.after`
 * appends `<wiki>` context blocks to the tool result text: residual
 * diagnostics, or the loud SKIPPED notice when the binary cannot be launched
 * (surfacing parity with the claude/codex adapters).
 *
 * Fail-open by contract: the whole after-hook body is guarded because an
 * uncaught error would surface on a fail-closed host loader — a missed
 * advisory must never block an already-executed tool call.
 */

import { isAbsolute, resolve as resolvePath } from 'node:path';
import {
  extractPatchedFilePaths,
  isWikiFile,
  resolveWikiBinary,
  runWikiCheck,
  type WikiCheckResult,
  wikiContextBlock,
  wikiUnavailableBlock
} from '../common/wiki-check.js';
import type { OpencodeAfterOutput, OpencodePluginInput, OpencodeToolInput, WikiOpencodeHooks } from './types.js';

export type { OpencodeAfterOutput, OpencodePluginInput, OpencodeToolInput, WikiOpencodeHooks } from './types.js';

/** Tool ids whose writes can touch wiki members (no notebook concept on opencode). */
const WRITE_TOOL_IDS = new Set(['edit', 'write']);
const PATCH_TOOL_ID = 'apply_patch';

const WIKI_CHECK_TIMEOUT_MS = 25000;

/** Injected surfaces for {@link assemblePlugin} — every field optional. */
export interface PluginDeps {
  /** Directory opencode resolved the plugin in; defaults to process cwd. */
  directory?: string;
  /** Binary resolution override; defaults to the core's resolver. */
  resolveBinary?: () => string;
  /** Check-executor override; defaults to the real {@link runWikiCheck}. */
  executeCheck?: (filePath: string, options: { binary: string }) => WikiCheckResult;
}

/** Narrow the unknown tool-call args to a usable file path, resolved against the directory. */
function narrowFilePath(args: unknown, directory: string): string | null {
  if (args === null || typeof args !== 'object' || !('filePath' in args)) return null;
  const raw = (args as { filePath: unknown }).filePath;
  if (typeof raw !== 'string' || raw.length === 0) return null;
  return isAbsolute(raw) ? raw : resolvePath(directory, raw);
}

/** Narrow the apply_patch tool-call args ({patchText}, gpt-models only). */
function narrowPatchTextArgs(args: unknown): string | null {
  if (args === null || typeof args !== 'object' || !('patchText' in args)) return null;
  const raw = (args as { patchText: unknown }).patchText;
  if (typeof raw !== 'string' || raw.length === 0) return null;
  return raw;
}

/** Candidate target paths for one tool call, before the wiki-membership gate. */
function candidatePaths(toolId: string, args: unknown, directory: string): string[] {
  if (toolId === PATCH_TOOL_ID) {
    const patchText = narrowPatchTextArgs(args);
    return patchText === null ? [] : extractPatchedFilePaths(patchText);
  }
  const filePath = narrowFilePath(args, directory);
  return filePath === null ? [] : [filePath];
}

/**
 * Build the hooks object over injected dependencies. The default export calls
 * this with production defaults; tests call it with an injected binary
 * resolver or executor while still exercising the real implementations.
 */
export function assemblePlugin(deps: PluginDeps = {}): WikiOpencodeHooks {
  const directory = deps.directory ?? process.cwd();
  const resolveBinary = deps.resolveBinary ?? (() => resolveWikiBinary());
  const executeCheck =
    deps.executeCheck ??
    ((filePath: string, options: { binary: string }) =>
      runWikiCheck(filePath, { binary: options.binary, timeoutMs: WIKI_CHECK_TIMEOUT_MS, cwd: directory }));

  const afterHandler = async (input?: OpencodeToolInput, output?: OpencodeAfterOutput): Promise<void> => {
    try {
      if (input === null || typeof input !== 'object') return;
      const toolId = typeof input.tool === 'string' ? input.tool : '';
      if (toolId !== PATCH_TOOL_ID && !WRITE_TOOL_IDS.has(toolId)) return;

      const paths = candidatePaths(toolId, input.args, directory).map((p) =>
        isAbsolute(p) ? p : resolvePath(directory, p)
      );
      // Frontmatter gate first, so silence means either "not a wiki member" or
      // "validated" — never an unnoticed skip.
      const wikiPaths = paths.filter((p) => isWikiFile(p, directory));
      if (wikiPaths.length === 0) return;

      const wikiBin = resolveBinary();

      // Single pass over every touched wiki member: --fix auto-repairs drift in
      // place; non-zero exits mean residual conditions the agent must resolve.
      const sections: string[] = [];
      let unavailableDetail: string | null = null;
      for (const filePath of wikiPaths) {
        const result = executeCheck(filePath, { binary: wikiBin });
        if (result.status === 'unavailable') {
          unavailableDetail ??= result.output ?? 'spawn failed';
          continue;
        }
        if (result.status === 'residual' && result.output) sections.push(result.output);
      }
      if (sections.length === 0 && unavailableDetail === null) return;
      if (output === null || typeof output !== 'object') return;

      // Unavailable wins over collected residuals, mirroring the codex adapter:
      // an unlaunched binary invalidates any partial sibling diagnostics too.
      const payload =
        unavailableDetail !== null
          ? wikiUnavailableBlock(wikiPaths[0], wikiBin, unavailableDetail)
          : wikiContextBlock(sections.join('\n\n'));
      const prior = typeof output.output === 'string' ? output.output : '';
      output.output = `${prior}\n${payload}`;
    } catch {
      // Fail-open: swallow everything; see module doc.
    }
  };

  return {
    'tool.execute.after': async (input, output) => {
      await afterHandler(input, output);
    },
    dispose: () => {}
  };
}

/**
 * The plugin OpenCode loads: an async init receiving `{ directory }` and
 * returning the hooks object. Never rejects into the fail-closed host loader.
 */
export async function wikiOpencode(input?: OpencodePluginInput): Promise<WikiOpencodeHooks> {
  return assemblePlugin({
    directory: typeof input?.directory === 'string' && input.directory.length > 0 ? input.directory : undefined
  });
}

/**
 * The module shape OpenCode's loader detects: a default-exported
 * `{ id?, server }` object whose `server` builds the hooks. Detection on the
 * `server` key short-circuits before the host inspects named exports that are
 * helpers rather than plugin initializer functions.
 *
 * The id matches the npm package name so path and registry installations have
 * identical attribution in host logs.
 */
const pluginModule: { id: string; server: typeof wikiOpencode } = {
  id: '@goodfoot/opencode-wiki',
  server: wikiOpencode
};

export default pluginModule;
