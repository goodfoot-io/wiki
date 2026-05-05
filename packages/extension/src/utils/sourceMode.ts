/**
 * Source mode configuration helper for wiki CLI commands.
 *
 * Reads the `wiki.sourceMode` setting and returns the `--source <mode>` CLI
 * arguments when the mode is non-default ("worktree"). This enables pre-commit
 * and HEAD-comparison views without dirtying the worktree.
 *
 * @summary Source mode configuration helper for wiki CLI commands.
 */

import * as vscode from 'vscode';

/**
 * Read the `wiki.sourceMode` configuration and return the corresponding
 * `--source` CLI arguments.
 *
 * @returns An empty array when mode is "worktree" (default), or
 *          `['--source', 'index']` / `['--source', 'head']` for
 *          non-default modes.
 */
export function getSourceArgs(): string[] {
  const mode = vscode.workspace.getConfiguration('wiki').get<string>('sourceMode', 'worktree');
  if (mode === 'index') return ['--source', 'index'];
  if (mode === 'head') return ['--source', 'head'];
  return [];
}
