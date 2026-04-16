/**
 * Managed wiki CLI resolution, installation, and process-spawning helpers.
 *
 * Handles release asset downloads, checksum verification, managed storage
 * layout, PATH fallback for development, and absolute-path process execution.
 *
 * @summary Managed wiki CLI resolution, installation, and process helpers.
 */

import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants as fsConstants } from 'node:fs';
import { access, chmod, mkdir, readFile, rename, rm, stat, writeFile } from 'node:fs/promises';
import * as path from 'node:path';
import {
  getManagedBinaryPaths,
  getWikiChecksumsAssetName,
  getWikiReleaseTag,
  resolveWikiPlatform
} from './wikiPlatform.js';

export interface WikiBinaryHandle {
  path: string;
  source: 'managed' | 'path';
  version?: string;
}

export type WikiBinaryResolution =
  | { kind: 'managed'; path: string; version: string }
  | { kind: 'path'; path: string }
  | { kind: 'missing'; reason: string };

export interface WikiCommandResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

export interface WikiChecksumsManifest {
  version: string;
  assets: Record<string, { name: string; sha256: string }>;
}

interface ManagedBinaryManifest {
  version: string;
  platform: NodeJS.Platform;
  arch: NodeJS.Architecture;
  assetName: string;
  checksum: string;
  sourceUrl: string;
  installedAt: string;
}

export interface InstallManagedWikiBinaryParams {
  storageRoot: string;
  version: string;
  releaseBaseUrl: string;
  platform?: NodeJS.Platform;
  arch?: NodeJS.Architecture;
  fetchImpl?: typeof fetch;
}

export interface InstallManagedWikiBinaryResult {
  handle: WikiBinaryHandle;
  installed: boolean;
}

export class WikiBinaryError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'WikiBinaryError';
  }
}

/**
 * Normalize an unknown binary-resolution failure into a user-facing message.
 *
 * @param error - Error thrown while resolving or installing the binary.
 * @returns Human-readable error message.
 */
export function getWikiBinaryErrorMessage(error: unknown): string {
  if (error instanceof WikiBinaryError) {
    return error.message;
  }
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
}

/**
 * Validate an already-installed managed wiki binary for the current version.
 *
 * @param params - Managed binary lookup parameters.
 * @returns Managed binary handle when the install is valid, otherwise null.
 */
export async function resolveManagedWikiBinary(
  params: InstallManagedWikiBinaryParams
): Promise<WikiBinaryHandle | null> {
  const target = resolveWikiPlatform(params.platform, params.arch);
  if (target == null) {
    return null;
  }

  const managedPaths = getManagedBinaryPaths(params.storageRoot, params.version, target);
  let manifestBody: string;
  try {
    manifestBody = await readFile(managedPaths.manifestPath, 'utf8');
  } catch {
    return null;
  }

  let manifest: ManagedBinaryManifest;
  try {
    manifest = JSON.parse(manifestBody) as ManagedBinaryManifest;
  } catch {
    return null;
  }

  const expectedSourceUrl = `${normalizeReleaseBaseUrl(params.releaseBaseUrl)}/${getWikiReleaseTag(params.version)}/${target.assetName}`;
  if (
    manifest.version !== params.version ||
    manifest.platform !== target.platform ||
    manifest.arch !== target.arch ||
    manifest.assetName !== target.assetName ||
    manifest.sourceUrl !== expectedSourceUrl
  ) {
    return null;
  }

  try {
    await assertExecutable(managedPaths.binaryPath, target.platform);
  } catch {
    return null;
  }

  if ((await sha256File(managedPaths.binaryPath)) !== manifest.checksum) {
    return null;
  }

  return { path: managedPaths.binaryPath, source: 'managed', version: params.version };
}

/**
 * Download, verify, and install the managed wiki binary for the current version.
 *
 * @param params - Installation parameters including version, storage root, and release URL.
 * @returns Installed or previously validated managed binary handle.
 */
export async function installManagedWikiBinary(
  params: InstallManagedWikiBinaryParams
): Promise<InstallManagedWikiBinaryResult> {
  const fetchImpl = params.fetchImpl ?? fetch;
  const target = resolveWikiPlatform(params.platform, params.arch);
  if (target == null) {
    throw new WikiBinaryError(
      `wiki is not available for ${params.platform ?? process.platform}-${params.arch ?? process.arch} in this release.`
    );
  }

  const existing = await resolveManagedWikiBinary(params);
  if (existing != null) {
    return { handle: existing, installed: false };
  }

  const releaseBaseUrl = normalizeReleaseBaseUrl(params.releaseBaseUrl);
  const tag = getWikiReleaseTag(params.version);
  const checksumsUrl = `${releaseBaseUrl}/${tag}/${getWikiChecksumsAssetName()}`;
  const checksumsResponse = await fetchImpl(checksumsUrl);
  if (!checksumsResponse.ok) {
    throw new WikiBinaryError(
      `Failed to download wiki CLI checksums manifest from ${checksumsUrl} (HTTP ${checksumsResponse.status}).`
    );
  }

  const checksumsManifest = (await checksumsResponse.json()) as WikiChecksumsManifest;
  const asset = checksumsManifest.assets[target.assetKey];
  if (asset == null || asset.name !== target.assetName) {
    throw new WikiBinaryError(`Release manifest ${checksumsUrl} does not contain ${target.assetKey}.`);
  }

  const managedPaths = getManagedBinaryPaths(params.storageRoot, params.version, target);
  await mkdir(managedPaths.binaryDirectory, { recursive: true });
  await mkdir(managedPaths.manifestDirectory, { recursive: true });

  const assetUrl = `${releaseBaseUrl}/${tag}/${asset.name}`;
  const assetResponse = await fetchImpl(assetUrl);
  if (!assetResponse.ok) {
    throw new WikiBinaryError(`Failed to download wiki CLI asset from ${assetUrl} (HTTP ${assetResponse.status}).`);
  }

  const assetBytes = Buffer.from(await assetResponse.arrayBuffer());
  if (createHash('sha256').update(assetBytes).digest('hex') !== asset.sha256) {
    throw new WikiBinaryError(`Checksum verification failed for ${asset.name}.`);
  }

  const binaryDownloadPath = `${managedPaths.binaryPath}.download`;
  const manifestDownloadPath = `${managedPaths.manifestPath}.download`;
  await cleanupInstallArtifacts(binaryDownloadPath, manifestDownloadPath);
  await rm(managedPaths.binaryPath, { force: true });

  try {
    await writeFile(binaryDownloadPath, assetBytes);
    if (target.platform !== 'win32') {
      await chmod(binaryDownloadPath, 0o755);
    }
    await rename(binaryDownloadPath, managedPaths.binaryPath);
    await writeFile(
      manifestDownloadPath,
      `${JSON.stringify(
        {
          version: params.version,
          platform: target.platform,
          arch: target.arch,
          assetName: target.assetName,
          checksum: asset.sha256,
          sourceUrl: assetUrl,
          installedAt: new Date().toISOString()
        } satisfies ManagedBinaryManifest,
        null,
        2
      )}\n`
    );
    await rename(manifestDownloadPath, managedPaths.manifestPath);
  } catch (error) {
    await cleanupInstallArtifacts(binaryDownloadPath, manifestDownloadPath);
    await rm(managedPaths.binaryPath, { force: true });
    throw error;
  }

  return {
    handle: { path: managedPaths.binaryPath, source: 'managed', version: params.version },
    installed: true
  };
}

/**
 * Locate a wiki binary on PATH for explicit development fallback scenarios.
 *
 * @param platform - Host platform used for executable name resolution.
 * @param envPath - PATH value to search.
 * @returns PATH binary handle when present, otherwise null.
 */
export async function resolveWikiBinaryOnPath(
  platform: NodeJS.Platform = process.platform,
  envPath: string = process.env['PATH'] ?? ''
): Promise<WikiBinaryHandle | null> {
  const candidate = await findExecutableOnPath(platform === 'win32' ? 'wiki.exe' : 'wiki', platform, envPath);
  return candidate == null ? null : { path: candidate, source: 'path' };
}

/**
 * Spawn the resolved wiki CLI by absolute path and capture its output.
 *
 * @param binaryPath - Absolute path to the wiki executable.
 * @param args - CLI arguments to pass through.
 * @param signal - Optional AbortSignal to cancel the running process.
 * @param cwd - Optional working directory for the wiki process.
 * @returns Command stdout, stderr, and exit code.
 */
export function runWikiCommand(
  binaryPath: string,
  args: string[],
  signal?: AbortSignal,
  cwd?: string
): Promise<WikiCommandResult> {
  return new Promise((resolve, reject) => {
    const child = spawn(binaryPath, args, { stdio: ['ignore', 'pipe', 'pipe'], cwd });
    let stdout = '';
    let stderr = '';

    child.stdout.on('data', (chunk: Buffer) => {
      stdout += chunk.toString('utf-8');
    });
    child.stderr.on('data', (chunk: Buffer) => {
      stderr += chunk.toString('utf-8');
    });
    child.on('error', (error) => {
      reject(error);
    });
    child.on('close', (code) => {
      resolve({ stdout, stderr, exitCode: code ?? 1 });
    });

    if (signal != null) {
      const onAbort = () => child.kill();
      signal.addEventListener('abort', onAbort, { once: true });
      child.on('close', () => signal.removeEventListener('abort', onAbort));
    }
  });
}

async function cleanupInstallArtifacts(binaryDownloadPath: string, manifestDownloadPath: string): Promise<void> {
  await Promise.all([rm(binaryDownloadPath, { force: true }), rm(manifestDownloadPath, { force: true })]);
}

async function assertExecutable(filePath: string, platform: NodeJS.Platform): Promise<void> {
  await stat(filePath);
  if (platform === 'win32') {
    await access(filePath, fsConstants.F_OK);
    return;
  }
  await access(filePath, fsConstants.X_OK);
}

async function sha256File(filePath: string): Promise<string> {
  return createHash('sha256')
    .update(await readFile(filePath))
    .digest('hex');
}

async function findExecutableOnPath(
  executableName: string,
  platform: NodeJS.Platform,
  envPath: string
): Promise<string | null> {
  const directories = envPath
    .split(path.delimiter)
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0);

  const windowsExts = (process.env['PATHEXT'] ?? '.EXE;.CMD;.BAT;.COM').split(';').map((entry) => entry.toLowerCase());

  for (const directory of directories) {
    if (platform === 'win32') {
      const base = executableName.endsWith('.exe') ? executableName.slice(0, -4) : executableName;
      for (const extension of windowsExts) {
        const candidate = path.join(directory, `${base}${extension}`);
        try {
          await access(candidate, fsConstants.F_OK);
          return candidate;
        } catch {
          // Continue searching.
        }
      }
      continue;
    }

    const candidate = path.join(directory, executableName);
    try {
      await access(candidate, fsConstants.X_OK);
      return candidate;
    } catch {
      // Continue searching.
    }
  }

  return null;
}

function normalizeReleaseBaseUrl(releaseBaseUrl: string): string {
  return releaseBaseUrl.replace(/\/+$/, '');
}
