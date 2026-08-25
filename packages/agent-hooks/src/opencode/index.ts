/**
 * OpenCode plugin entry — wires the shared wiki-check core into the host's
 * `Hooks` shape. OpenCode loads this module in-process via file:// and calls
 * the default export once at startup; the returned `tool.execute.after`
 * appends `<wiki>` context blocks to the tool result text.
 *
 * Fail-open by contract: the whole after-hook body is guarded because an
 * uncaught error would surface on a fail-closed host loader — a missed
 * advisory must never block an already-executed tool call.
 */

import { isAbsolute, resolve as resolvePath } from 'node:path';
import { isWikiFile, resolveWikiBinary, runWikiCheck, type WikiCheckResult } from '../common/wiki-check.js';
import type { OpencodeAfterOutput, OpencodePluginInput, OpencodeToolInput, WikiOpencodeHooks } from './types.js';

export type { OpencodeAfterOutput, OpencodePluginInput, OpencodeToolInput, WikiOpencodeHooks } from './types.js';

/** Tool ids whose writes can touch wiki members (no notebook concept on opencode). */
const EDIT_TOOL_IDS = new Set(['edit', 'write']);

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

function wikiContextBlock(output: string): string {
  return `<wiki>\n${output}\n</wiki>`;
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
      if (!EDIT_TOOL_IDS.has(toolId)) return;

      const filePath = narrowFilePath(input.args, directory);
      if (filePath === null) return;
      if (!isWikiFile(filePath, directory)) return;

      const result = executeCheck(filePath, { binary: resolveBinary() });
      // Residual diagnostics only: a clean pass injected nothing, and an
      // unavailable binary fails open silently on this platform — the hook
      // cannot block, so there is no loud channel to skip through.
      if (result.status !== 'residual') return;
      const diagnostics = result.output;
      if (!diagnostics) return;
      if (output === null || typeof output !== 'object') return;

      const prior = typeof output.output === 'string' ? output.output : '';
      output.output = `${prior}\n${wikiContextBlock(diagnostics)}`;
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
export default async function wikiOpencode(input?: OpencodePluginInput): Promise<WikiOpencodeHooks> {
  return assemblePlugin({
    directory: typeof input?.directory === 'string' && input.directory.length > 0 ? input.directory : undefined
  });
}
