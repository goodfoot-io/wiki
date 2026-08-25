#!/usr/bin/env node
/**
 * opencode-wiki installer — materializes the plugin's wiki skill into
 * OpenCode's filesystem skill directories (npm plugins cannot contribute
 * skills directly).
 */

import { cpSync, existsSync, statSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const USAGE = `opencode-wiki — install the wiki skill for OpenCode

Usage:
  npx @goodfoot/opencode-wiki install [--global]

Options:
  --global   Install into $XDG_CONFIG_HOME/opencode (else ~/.config/opencode)
             instead of <cwd>/.opencode/
  -h, --help Show this help

Copies:
  skills/wiki -> <target>/skills/wiki/`;

function fail(message) {
  console.error(`ERROR: ${message}`);
  process.exit(1);
}

function parseArgs(argv) {
  let global = false;
  let command = undefined;
  for (const arg of argv) {
    if (arg === '--global') {
      global = true;
    } else if (arg === '-h' || arg === '--help') {
      console.log(USAGE);
      process.exit(0);
    } else if (arg === 'install' && command === undefined) {
      command = 'install';
    } else {
      fail(`unexpected argument "${arg}"\n\n${USAGE}`);
    }
  }
  return { command, global };
}

/**
 * OpenCode's own global discovery resolves its config directory XDG-first:
 * `$XDG_CONFIG_HOME/opencode` when the variable is set and non-empty, else
 * `~/.config/opencode`.
 */
function globalTarget() {
  const xdg = process.env.XDG_CONFIG_HOME;
  const base = typeof xdg === 'string' && xdg.length > 0 ? xdg : join(homedir(), '.config');
  return resolve(base, 'opencode');
}

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const { command, global } = parseArgs(process.argv.slice(2));

if (command !== 'install') {
  fail(`missing required "install" command\n\n${USAGE}`);
}

const target = global ? globalTarget() : resolve(process.cwd(), '.opencode');
const source = join(packageRoot, 'skills', 'wiki');

if (!existsSync(source)) {
  fail(`wiki skill not found at ${source} — the package installation looks broken`);
}
if (!statSync(source).isDirectory()) {
  fail(`wiki skill path is not a directory: ${source}`);
}

const destination = join(target, 'skills', 'wiki');
try {
  cpSync(source, destination, { recursive: true, dereference: true });
} catch (err) {
  fail(`failed to copy ${source} -> ${destination}: ${err instanceof Error ? err.message : String(err)}`);
}
console.log(`installed ${destination}`);
console.log(`opencode-wiki: wiki skill installed into ${target}`);
