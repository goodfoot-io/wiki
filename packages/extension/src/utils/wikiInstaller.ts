/**
 * VS Code-facing wiki CLI manager.
 *
 * Bridges extension activation, managed binary installation, terminal PATH
 * injection, and explicit development PATH fallback.
 *
 * @summary VS Code-facing wiki CLI manager.
 */

import * as path from 'node:path';
import * as vscode from 'vscode';
import {
  getWikiBinaryErrorMessage,
  type InstallManagedWikiBinaryResult,
  installManagedWikiBinary,
  resolveManagedWikiBinary,
  resolveWikiBinaryOnPath,
  WikiBinaryError,
  type WikiBinaryHandle
} from './wikiBinary.js';
import { resolveWikiPlatform } from './wikiPlatform.js';

interface WikiBinaryReadyResult {
  handle: WikiBinaryHandle;
  installed: boolean;
}

const DEFAULT_RELEASE_BASE_URL = 'https://github.com/goodfoot-io/wiki/releases/download';

export class WikiBinaryManager {
  private readyPromise: Promise<WikiBinaryReadyResult> | null = null;

  constructor(private readonly context: vscode.ExtensionContext) {}

  start(): Promise<WikiBinaryReadyResult> {
    this.readyPromise ??= this.ensureReady();
    return this.readyPromise;
  }

  async ready(): Promise<WikiBinaryHandle> {
    return (await this.start()).handle;
  }

  retry(): Promise<WikiBinaryReadyResult> {
    this.readyPromise = null;
    return this.start();
  }

  formatFailure(error: unknown): string {
    return `${getWikiBinaryErrorMessage(error)} Run "Wiki: Retry CLI Install" and try again.`;
  }

  private async ensureReady(): Promise<WikiBinaryReadyResult> {
    const version = this.extensionVersion();
    const releaseBaseUrl = this.releaseBaseUrl();
    const storageRoot = this.context.globalStorageUri.fsPath;

    const managed = await resolveManagedWikiBinary({ storageRoot, version, releaseBaseUrl });
    if (managed != null) {
      this.configureTerminalPath(path.dirname(managed.path));
      return { handle: managed, installed: false };
    }

    if (this.shouldUsePathFallback()) {
      const pathBinary = await resolveWikiBinaryOnPath();
      if (pathBinary != null) {
        return { handle: pathBinary, installed: false };
      }
    }

    const target = resolveWikiPlatform();
    if (target == null) {
      throw new WikiBinaryError(`wiki is not available for ${process.platform}-${process.arch} in this release.`);
    }

    const installed = await installManagedWikiBinary({
      storageRoot,
      version,
      releaseBaseUrl,
      platform: target.platform,
      arch: target.arch
    });
    this.configureTerminalPath(path.dirname(installed.handle.path));
    return installed;
  }

  private extensionVersion(): string {
    const pkg = this.context.extension.packageJSON as { version?: string };
    const version = pkg.version;
    if (typeof version !== 'string' || version.length === 0) {
      throw new WikiBinaryError('Extension package.json is missing a version.');
    }
    return version;
  }

  private releaseBaseUrl(): string {
    const envOverride = process.env['WIKI_EXTENSION_RELEASE_BASE_URL'];
    if (envOverride != null && envOverride.length > 0) {
      return envOverride;
    }
    return vscode.workspace.getConfiguration('wiki').get<string>('binary.releaseBaseUrl', DEFAULT_RELEASE_BASE_URL);
  }

  private shouldUsePathFallback(): boolean {
    const envOverride = process.env['WIKI_EXTENSION_USE_PATH_FALLBACK'];
    if (envOverride != null) {
      return envOverride === '1' || envOverride.toLowerCase() === 'true';
    }
    if (this.context.extensionMode === vscode.ExtensionMode.Development) {
      return true;
    }
    return vscode.workspace.getConfiguration('wiki').get<boolean>('binary.usePathFallback', false);
  }

  private configureTerminalPath(binDirectory: string): void {
    const collection = this.context.environmentVariableCollection;
    collection.clear();
    collection.description = 'Adds the managed wiki CLI to integrated terminal PATH.';
    collection.persistent = true;
    collection.prepend('PATH', `${binDirectory}${path.delimiter}`);
  }
}

/**
 * Return true when a binary-manager resolution performed a fresh managed install.
 *
 * @param result - Binary-manager start or install result.
 * @returns Whether a new managed binary was installed.
 */
export function wasManagedInstall(result: InstallManagedWikiBinaryResult | WikiBinaryReadyResult): boolean {
  return result.handle.source === 'managed' && result.installed;
}
