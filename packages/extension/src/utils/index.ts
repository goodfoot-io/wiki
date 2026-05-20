/**
 * Shared utilities used across the wiki extension: YAML frontmatter parsing
 * for wiki-aware file detection, the singleton "Wiki" `LogOutputChannel`
 * shared by every host caller, install/spawn helpers for the managed `wiki`
 * CLI, the per-workspace recently-viewed-articles MRU surfaced by the
 * QuickPick, and the platform-target resolver that selects the right
 * prebuilt binary for the current host.
 *
 * @summary Cross-cutting host utilities — frontmatter, logger, binary install/spawn, MRU, platform target.
 */

export * from './frontmatter.js';
export * from './logger.js';
export * from './recentlyViewed.js';
export * from './wikiBinary.js';
export * from './wikiInstaller.js';
export * from './wikiPlatform.js';
