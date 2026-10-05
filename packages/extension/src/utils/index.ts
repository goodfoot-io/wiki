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

export { type FrontmatterInfo, readFrontmatter } from './frontmatter.js';
export { formatLogError, getWikiLogger, registerWikiLogger } from './logger.js';
export { loadValidatedRecentlyViewed, recordWikiView } from './recentlyViewed.js';
export { runWikiCommand } from './wikiBinary.js';
export { WikiBinaryManager, wasManagedInstall } from './wikiInstaller.js';
