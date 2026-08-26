#!/usr/bin/env node
/**
 * Wraps the unified `@goodfoot/agent-hooks` CLI so every invocation -- via
 * `yarn build:hooks`/`yarn build:hooks:codex` or directly via `yarn
 * agent-hooks-cli` -- gets its generated `.mjs` output normalized afterward
 * by `scripts/normalize-hook-module-comments.js`.
 *
 * It also converts each manifest's hook registration `timeout` from
 * milliseconds to seconds -- but only for `--agent claude-code`, which emits
 * the source value verbatim even though Claude Code reads the field as
 * seconds ("Seconds before canceling"). `--agent codex` performs the same
 * division itself (`timeoutMsToSeconds`) before writing its manifest, so its
 * output must pass through untouched; re-dividing it would shrink every
 * ceiling a thousandfold. Source authoring stays milliseconds across both
 * adapters, matching the codex pipeline.
 *
 * This package's package.json defines an "agent-hooks-cli" script that
 * points here. Because Yarn resolves a bare `yarn <name>` invocation against
 * package.json scripts *before* falling back to a same-named binary
 * contributed by a dependency, `yarn agent-hooks-cli ...` runs this wrapper
 * instead of the raw CLI.
 *
 * The upstream CLI already fails closed (stderr message, exit 1, no file
 * written) for a missing or unknown `--agent` value, and for the unparsed
 * `--agent=value` equals form (which it silently ignores, leaving `--agent`
 * effectively missing). The one gap it leaves open is a *repeated* `--agent`
 * flag with conflicting values -- it keeps only the last occurrence rather
 * than rejecting the ambiguity -- so this wrapper validates only that case
 * before spawning anything.
 *
 * The wrapper forwards all CLI args unchanged to the real, installed CLI
 * (resolved by realpath through node_modules -- symlinked or not -- so the
 * actual compiled output is byte-for-byte what the CLI itself produces),
 * then post-processes only the `//` module-boundary comments in the
 * directory the `-o`/`--output` argument points at.
 *
 * Usage: node scripts/hooks-cli-wrapper.js --agent <claude-code|codex> [...cli-args]
 */

import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { basename, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { normalizeModuleComments } from './normalize-hook-module-comments.js';

function findOutputPath(cliArgs) {
  for (let i = 0; i < cliArgs.length; i += 1) {
    const arg = cliArgs[i];
    if (arg === '-o' || arg === '--output') {
      return cliArgs[i + 1];
    }
    if (arg.startsWith('--output=')) {
      return arg.slice('--output='.length);
    }
    if (arg.startsWith('-o=')) {
      return arg.slice('-o='.length);
    }
  }
  return undefined;
}

// Only rejects a repeated `--agent` flag with conflicting values -- the one
// selector-ambiguity case the upstream CLI's own validateArgs() does not
// catch (it keeps only the last occurrence). A single occurrence, a missing
// one, repeated occurrences of the same value, and the unparsed `--agent=`
// equals form are all left to upstream, which already fails closed on each
// before writing any file.
function findConflictingAgentValues(cliArgs) {
  const values = [];
  for (let i = 0; i < cliArgs.length; i += 1) {
    if (cliArgs[i] === '--agent') {
      values.push(cliArgs[i + 1]);
    }
  }
  const distinct = new Set(values);
  return distinct.size > 1 ? [...distinct] : undefined;
}

function findInputPath(cliArgs) {
  for (let i = 0; i < cliArgs.length; i += 1) {
    const arg = cliArgs[i];
    if (arg === '-i' || arg === '--input') {
      return cliArgs[i + 1];
    }
    if (arg.startsWith('--input=')) {
      return arg.slice('--input='.length);
    }
    if (arg.startsWith('-i=')) {
      return arg.slice('-i='.length);
    }
  }
  return undefined;
}

function declaredBundleOrder(inputArg) {
  if (inputArg === undefined) return [];
  const brace = inputArg.match(/\{([^{}]+)\}/);
  const paths =
    brace === null
      ? [inputArg]
      : brace[1]
          .split(',')
          .map((part) => `${inputArg.slice(0, brace.index)}${part}${inputArg.slice(brace.index + brace[0].length)}`);
  return paths.map((path) => basename(path).replace(/\.[^.]+$/, ''));
}

function commandBundle(command) {
  return command.match(/([A-Za-z0-9-]+)\.mjs\b/)?.[1];
}

function canonicalizeHookManifest(outputPath, inputArg) {
  const manifest = JSON.parse(readFileSync(outputPath, 'utf8'));
  const declared = declaredBundleOrder(inputArg);
  const rankByBundle = new Map(declared.map((bundle, index) => [bundle, index]));
  const rankOfCommand = (command) => rankByBundle.get(commandBundle(command)) ?? Number.MAX_SAFE_INTEGER;
  const rankOfGroup = (group) => Math.min(...(group.hooks ?? []).map((hook) => rankOfCommand(hook.command)));
  const compareGroups = (left, right) => {
    const rank = rankOfGroup(left) - rankOfGroup(right);
    if (rank !== 0) return rank;
    return JSON.stringify(left).localeCompare(JSON.stringify(right));
  };

  const hookEntries = Object.entries(manifest.hooks ?? {});
  for (const [, groups] of hookEntries) {
    for (const group of groups) {
      group.hooks?.sort((left, right) => {
        const rank = rankOfCommand(left.command) - rankOfCommand(right.command);
        return rank !== 0 ? rank : left.command.localeCompare(right.command);
      });
    }
    groups.sort(compareGroups);
  }
  hookEntries.sort(([leftEvent, leftGroups], [rightEvent, rightGroups]) => {
    const leftRank = Math.min(...leftGroups.map(rankOfGroup));
    const rightRank = Math.min(...rightGroups.map(rankOfGroup));
    return leftRank !== rightRank ? leftRank - rightRank : leftEvent.localeCompare(rightEvent);
  });
  manifest.hooks = Object.fromEntries(hookEntries);

  // The CLI stamps build wall-clock time here. It tries to
  // preserve a prior value when the generated file set is unchanged, but any
  // rebuild without an existing manifest emits a fresh one -- churning the
  // committed artifact and tripping the freshness gate. Strip it so emitted
  // bytes depend only on inputs.
  if (manifest.__generated && 'timestamp' in manifest.__generated) {
    delete manifest.__generated.timestamp;
  }

  if (Array.isArray(manifest.__generated?.files)) {
    manifest.__generated.files.sort((left, right) => {
      const leftRank = rankByBundle.get(left.replace(/\.mjs$/, '')) ?? Number.MAX_SAFE_INTEGER;
      const rightRank = rankByBundle.get(right.replace(/\.mjs$/, '')) ?? Number.MAX_SAFE_INTEGER;
      return leftRank !== rightRank ? leftRank - rightRank : left.localeCompare(right);
    });
  }
  writeFileSync(outputPath, `${JSON.stringify(manifest, null, 2)}\n`);
}

// Mirrors the codex-hooks CLI's own emit-time conversion exactly, including
// the 1-second floor: a sub-second ms budget would otherwise round down to a
// zero-second (never-cancelling) registration.
function timeoutMsToSeconds(timeoutMs) {
  return Math.max(1, Math.ceil(timeoutMs / 1000));
}

function convertHookTimeoutsToSeconds(outputPath) {
  const manifest = JSON.parse(readFileSync(outputPath, 'utf8'));
  for (const groups of Object.values(manifest.hooks ?? {})) {
    for (const group of groups ?? []) {
      for (const hook of group.hooks ?? []) {
        if (typeof hook.timeout === 'number') {
          hook.timeout = timeoutMsToSeconds(hook.timeout);
        }
      }
    }
  }
  writeFileSync(outputPath, `${JSON.stringify(manifest, null, 2)}\n`);
}

async function main() {
  const cliArgs = process.argv.slice(2);

  const conflicting = findConflictingAgentValues(cliArgs);
  if (conflicting !== undefined) {
    process.stderr.write(
      `hooks-cli-wrapper: conflicting --agent values: ${conflicting.join(', ')}. Pass --agent exactly once.\n`
    );
    process.exit(1);
  }

  const cliEntryUrl = import.meta.resolve('@goodfoot/agent-hooks');
  const cliEntryPath = resolve(dirname(fileURLToPath(cliEntryUrl)), 'cli.js');

  const result = spawnSync(process.execPath, [cliEntryPath, ...cliArgs], {
    stdio: 'inherit'
  });

  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }

  const outputArg = findOutputPath(cliArgs);
  if (outputArg === undefined) {
    // Nothing was compiled to a hooks.json (e.g. --scaffold, --help); no
    // generated .mjs output to normalize.
    return;
  }
  const outputDir = dirname(resolve(process.cwd(), outputArg));
  const agentValue = cliArgs.findLast((_, i) => cliArgs[i - 1] === '--agent');
  if (agentValue === 'claude-code') {
    // Only --agent claude-code needs the division -- --agent codex already
    // emits seconds, and converting its manifest again would corrupt it.
    convertHookTimeoutsToSeconds(resolve(process.cwd(), outputArg));
  }
  normalizeModuleComments([outputDir]);
  canonicalizeHookManifest(resolve(process.cwd(), outputArg), findInputPath(cliArgs));
}

main().catch((error) => {
  process.stderr.write(`hooks-cli-wrapper: ${error instanceof Error ? error.message : String(error)}\n`);
  process.exit(1);
});
