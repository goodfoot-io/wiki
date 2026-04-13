/**
 * Utilities for locating and invoking the wiki CLI binary.
 *
 * Provides findWikiBinary() for PATH lookup and runWikiCommand() for spawning
 * the wiki process with stdout/stderr capture and optional AbortSignal support.
 *
 * @summary Utilities for locating and invoking the wiki CLI binary.
 */

import { execSync, spawn } from 'node:child_process';

/**
 * Attempt to locate the wiki binary on PATH.
 * Returns the absolute path if found, or null if not available.
 *
 * @returns The absolute path to the wiki binary, or null if not found.
 */
export function findWikiBinary(): string | null {
  try {
    return execSync('which wiki', { encoding: 'utf-8' }).trim();
  } catch {
    return null;
  }
}

/**
 * Result of running a wiki CLI command.
 */
export interface WikiCommandResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

/**
 * Run the wiki binary with the given arguments.
 * Resolves with stdout, stderr, and exit code.
 * If the AbortSignal fires before the process exits, the child process is killed.
 *
 * @param args - Arguments to pass to the wiki binary
 * @param signal - Optional AbortSignal to cancel the running process
 * @param cwd - Working directory for the wiki process; must be inside the git repo
 * @returns Promise resolving to stdout, stderr, and the process exit code.
 */
export function runWikiCommand(args: string[], signal?: AbortSignal, cwd?: string): Promise<WikiCommandResult> {
  return new Promise((resolve, reject) => {
    const child = spawn('wiki', args, { stdio: ['ignore', 'pipe', 'pipe'], cwd });

    let stdout = '';
    let stderr = '';

    child.stdout.on('data', (chunk: Buffer) => {
      stdout += chunk.toString('utf-8');
    });

    child.stderr.on('data', (chunk: Buffer) => {
      stderr += chunk.toString('utf-8');
    });

    child.on('error', (err) => {
      reject(err);
    });

    child.on('close', (code) => {
      resolve({ stdout, stderr, exitCode: code ?? 1 });
    });

    if (signal != null) {
      const onAbort = () => {
        child.kill();
      };
      signal.addEventListener('abort', onAbort, { once: true });
      child.on('close', () => {
        signal.removeEventListener('abort', onAbort);
      });
    }
  });
}
