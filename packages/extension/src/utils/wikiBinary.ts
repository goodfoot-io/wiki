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
import { getWikiLogger } from './logger.js';
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
  size: number;
}

/**
 * Module-level lock map keyed by `storageRoot:version:platform:arch`, ensuring
 * that concurrent `installManagedWikiBinary` calls for the same install target
 * share a single download/install promise rather than racing to the same paths.
 */
const inflightInstalls = new Map<string, Promise<InstallManagedWikiBinaryResult>>();

export interface InstallManagedWikiBinaryParams {
  storageRoot: string;
  version: string;
  releaseBaseUrl: string;
  platform?: NodeJS.Platform;
  arch?: NodeJS.Architecture;
  fetchImpl?: typeof fetch;
  signal?: AbortSignal;
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

  // Fast path: trust the binary when its mtime is not newer than the install
  // time and its size matches the manifest record.  An unchanged mtime + size
  // means the file has not been replaced or tampered with — there is no need
  // to re-read the entire binary for a full SHA-256 comparison.
  try {
    const binaryStat = await stat(managedPaths.binaryPath);
    const installedAtMs = new Date(manifest.installedAt).getTime();
    if (binaryStat.mtimeMs <= installedAtMs && binaryStat.size === manifest.size) {
      return { path: managedPaths.binaryPath, source: 'managed', version: params.version };
    }
  } catch {
    // stat failure (e.g. deleted between checks) — fall through to sha256
    getWikiLogger().trace(
      'resolveManagedWikiBinary: stat failed for %s, falling back to sha256',
      managedPaths.binaryPath
    );
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

  // Serialize concurrent installs for the same target: if another call is
  // already downloading, await its promise so both callers return with the
  // same result and only one download occurs.
  const installKey = `${params.storageRoot}:${params.version}:${target.platform}:${target.arch}`;
  const inflight = inflightInstalls.get(installKey);
  if (inflight) {
    return inflight;
  }

  // Fold the inflight-map cleanup into the single returned promise via
  // try/finally so it runs whether the install resolves or rejects, without
  // spawning a second promise chain.  A detached `.finally()`/`.then()` would
  // re-propagate the install's rejection into a floating promise that nobody
  // awaits, surfacing as an unhandled rejection; here the rejection flows
  // straight through this promise to the caller.
  const promise = (async () => {
    try {
      return await installManagedWikiBinaryInner(fetchImpl, params, target);
    } finally {
      inflightInstalls.delete(installKey);
    }
  })();
  inflightInstalls.set(installKey, promise);

  return promise;
}

/**
 * Inner download-and-install body, extracted so the outer function can
 * serialize concurrent calls via {@link inflightInstalls}.
 *
 * @param fetchImpl - Fetch implementation used for HTTP requests.
 * @param params - Installation parameters including version, storage root, and release URL.
 * @param target - Resolved platform target guaranteed non-null.
 * @returns Installed or previously validated managed binary handle.
 */
async function installManagedWikiBinaryInner(
  fetchImpl: typeof fetch,
  params: InstallManagedWikiBinaryParams,
  target: NonNullable<ReturnType<typeof resolveWikiPlatform>>
): Promise<InstallManagedWikiBinaryResult> {
  const releaseBaseUrl = normalizeReleaseBaseUrl(params.releaseBaseUrl);
  const tag = getWikiReleaseTag(params.version);
  const checksumsUrl = `${releaseBaseUrl}/${tag}/${getWikiChecksumsAssetName()}`;

  // Apply a 30-second default timeout so a hung connection never blocks
  // activation indefinitely.  When the caller provides their own signal we
  // combine the two so that either the caller's cancellation or the default
  // timeout can trigger first.
  const signal =
    params.signal != null ? AbortSignal.any([params.signal, AbortSignal.timeout(30_000)]) : AbortSignal.timeout(30_000);

  const checksumsResponse = await fetchWithAbort(fetchImpl, checksumsUrl, signal);
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
  const assetResponse = await fetchWithAbort(fetchImpl, assetUrl, signal);
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
          installedAt: new Date().toISOString(),
          size: assetBytes.length
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
 * Race a fetch call against an {@link AbortSignal} so the caller's promise
 * rejects when the signal fires, even when the fetch implementation itself
 * ignores the signal (e.g. in test mocks or non-standard implementations).
 *
 * @param fetchImpl - The fetch implementation to call.
 * @param url - URL to fetch.
 * @param signal - Abort signal to race against.
 * @returns The fetch response promise, rejected on abort.
 */
async function fetchWithAbort(fetchImpl: typeof fetch, url: string, signal: AbortSignal): Promise<Response> {
  // When the signal already fired before we could register the listener,
  // reject immediately — there is no point initiating the fetch.
  if (signal.aborted) {
    throw new DOMException('The operation was aborted', 'AbortError');
  }

  // Race the real fetch against a promise that rejects synchronously when
  // the signal fires.  This protects callers whose fetch implementation
  // does not check the signal option (e.g. test mocks).
  return new Promise<Response>((resolve, reject) => {
    const onAbort = () => reject(new DOMException('The operation was aborted', 'AbortError'));
    signal.addEventListener('abort', onAbort, { once: true });
    fetchImpl(url, { signal }).then(
      (response) => {
        signal.removeEventListener('abort', onAbort);
        resolve(response);
      },
      (error) => {
        signal.removeEventListener('abort', onAbort);
        reject(error);
      }
    );
  });
}

/**
 * Locate a wiki binary on PATH for explicit development fallback scenarios.
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
  const log = getWikiLogger().getChildLogger({ label: 'Spawn' });
  const startedAt = Date.now();
  // Insert '--' separator when a user-supplied query looks like a CLI flag,
  // preventing argument injection (e.g., typing '--help' in the QuickPick).
  if (args.length > 0 && args[0] != null && args[0].startsWith('-') && args[0] !== '--') {
    args = ['--', ...args];
  }
  log.debug('spawn %s %s (cwd=%s)', binaryPath, args.join(' '), cwd ?? '<inherit>');
  return new Promise((resolve, reject) => {
    const child = spawn(binaryPath, args, { stdio: ['ignore', 'pipe', 'pipe'], cwd });
    let stdout = '';
    let stderr = '';
    let stdoutBytes = 0;
    let stderrBytes = 0;

    child.stdout.on('data', (chunk: Buffer) => {
      stdoutBytes += chunk.length;
      stdout += chunk.toString('utf-8');
    });
    child.stderr.on('data', (chunk: Buffer) => {
      stderrBytes += chunk.length;
      stderr += chunk.toString('utf-8');
    });
    child.on('error', (error) => {
      log.error(
        'spawn error after %dms: %s (cmd=%s %s)',
        Date.now() - startedAt,
        error.message,
        binaryPath,
        args.join(' ')
      );
      reject(error);
    });
    child.on('close', (code) => {
      const duration = Date.now() - startedAt;
      const exitCode = code ?? 1;
      if (exitCode === 0) {
        log.debug('exit 0 in %dms (stdout=%dB stderr=%dB) cmd=%s', duration, stdoutBytes, stderrBytes, args.join(' '));
      } else if (signal?.aborted === true) {
        log.debug('aborted after %dms (cmd=%s)', duration, args.join(' '));
      } else {
        log.warn('exit %d in %dms cmd=%s stderr=%s', exitCode, duration, args.join(' '), stderr.trim().slice(0, 500));
      }
      resolve({ stdout, stderr, exitCode });
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
        } catch (err) {
          getWikiLogger().trace('PATH probe miss %s: %s', candidate, (err as Error).message);
        }
      }
      continue;
    }

    const candidate = path.join(directory, executableName);
    try {
      await access(candidate, fsConstants.X_OK);
      return candidate;
    } catch (err) {
      getWikiLogger().trace('PATH probe miss %s: %s', candidate, (err as Error).message);
    }
  }

  return null;
}

function normalizeReleaseBaseUrl(releaseBaseUrl: string): string {
  return releaseBaseUrl.replace(/\/+$/, '');
}
