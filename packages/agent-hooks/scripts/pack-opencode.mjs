#!/usr/bin/env node
/**
 * Packs the opencode wiki plugin into a publishable npm tarball.
 *
 * Stages a dereferenced copy of ../../plugins-opencode/wiki (symlinks become
 * real files, so the central skills tree travels inside the tarball) plus the
 * built dist/ bundle into a fresh temp directory, runs `npm pack` there, and
 * prints the absolute tarball path.
 *
 * Deterministic and network-free: the stage is built from checkout contents
 * only, and npm never consults the registry for a local pack.
 *
 * Usage: node scripts/pack-opencode.mjs
 */

import { spawnSync } from 'node:child_process';
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  rmSync,
  statSync
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), '../../../plugins-opencode/wiki');

if (!existsSync(join(packageDir, 'package.json'))) {
  process.stderr.write(`pack-opencode: missing ${join(packageDir, 'package.json')} — plugin tree not present\n`);
  process.exit(1);
}
if (!existsSync(join(packageDir, 'dist', 'index.js'))) {
  process.stderr.write('pack-opencode: dist/index.js missing — run `yarn build:opencode` first\n');
  process.exit(1);
}

// cpSync({ dereference: true }) does not reliably dereference symlinks to
// directories (skills/wiki lands as a dangling absolute link), and npm pack
// silently drops symlink entries from tarballs — so the staged skills tree
// would vanish from the published package. Walk manually: statSync follows
// symlinks, so every link is copied as the real file or directory it points
// at, with mode bits preserved so bin/ scripts stay executable.
function dereferencedCopy(source, destination) {
  const stats = statSync(source);
  if (stats.isDirectory()) {
    mkdirSync(destination, { recursive: true });
    for (const entry of readdirSync(source)) {
      dereferencedCopy(join(source, entry), join(destination, entry));
    }
    return;
  }
  if (stats.isFile()) {
    copyFileSync(source, destination);
    chmodSync(destination, stats.mode & 0o777);
  }
}

const stage = mkdtempSync(join(tmpdir(), 'opencode-wiki-pack-'));
// The stage intentionally persists: the printed tarball lives inside it, and
// OS temp cleanup reclaims the directory. Re-running creates a fresh stage.

try {
  dereferencedCopy(packageDir, stage);

  const pack = spawnSync('npm', ['pack', '--pack-destination', stage, '--json'], {
    cwd: stage,
    encoding: 'utf8'
  });
  if (pack.error) throw pack.error;
  if (pack.status !== 0) {
    process.stderr.write(pack.stderr ?? 'pack-opencode: npm pack failed\n');
    process.exit(pack.status ?? 1);
  }

  let tarballPath;
  try {
    const parsed = JSON.parse(pack.stdout);
    const entry = Array.isArray(parsed) ? parsed[0] : parsed;
    tarballPath = entry?.path ?? entry?.filename;
  } catch {
    const lines = (pack.stdout ?? '')
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean);
    tarballPath = lines[lines.length - 1];
  }
  if (!tarballPath) {
    process.stderr.write(`pack-opencode: could not determine tarball name from output:\n${pack.stdout}\n`);
    process.exit(1);
  }
  if (!existsSync(tarballPath)) {
    tarballPath = join(stage, tarballPath);
  }
  process.stdout.write(`${tarballPath}\n`);
} catch (error) {
  rmSync(stage, { recursive: true, force: true });
  throw error;
}
