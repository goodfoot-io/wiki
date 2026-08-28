import { existsSync, lstatSync, realpathSync } from 'node:fs';
import { dirname, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { isWikiFile } from '../../src/common/wiki-check.js';

/**
 * The wiki skill used to be one authored tree symlinked into every platform
 * location. `@goodfoot/agent-skills` now renders `skills-src/wiki` into each
 * tree as real files, so the invariant these tests hold is no longer "the link
 * resolves" but "the published tree is real, complete, and still carries the
 * frontmatter that makes its pages wiki pages".
 *
 * Page-hood is asserted against the *published* SKILL.md rather than the
 * template, because `it.frontmatter()` would have silently reduced it to
 * name/description. Keeping title+summary is what lets the same file serve as
 * both a skill entry point and a wiki page.
 */

const repoRoot = realpathSync(resolve(dirname(fileURLToPath(import.meta.url)), '../../../..'));

// Mirrors the `targets` array in scripts/agent-skills-plugins.json.
const generatedSkillTrees = [
  'plugins-claude/wiki/skills/wiki',
  'plugins-codex/wiki/skills/wiki',
  'plugins-opencode/wiki/skills/wiki',
  'plugins-antigravity/wiki/skills/wiki',
  'skills/wiki'
];

describe('generated wiki skill trees', () => {
  for (const treePath of generatedSkillTrees) {
    it(`${treePath} is a real published tree whose SKILL.md is a wiki page`, () => {
      const tree = resolve(repoRoot, treePath);
      expect(lstatSync(tree).isDirectory(), `${treePath} must be a real directory, not a symlink`).toBe(true);

      const skill = resolve(tree, 'SKILL.md');
      expect(existsSync(skill), `${treePath}/SKILL.md must exist`).toBe(true);
      expect(isWikiFile(skill, repoRoot), `${treePath}/SKILL.md must carry non-empty frontmatter title+summary`).toBe(
        true
      );
    });
  }

  it('.agents/skills/wiki still resolves inside the repo to the shared generated tree', () => {
    const link = resolve(repoRoot, '.agents/skills/wiki');
    expect(lstatSync(link).isSymbolicLink(), '.agents/skills/wiki must remain a symlink').toBe(true);

    const target = realpathSync(link);
    expect(target.startsWith(repoRoot + sep), `must resolve inside the repo, got ${target}`).toBe(true);
    expect(target).toBe(realpathSync(resolve(repoRoot, 'skills/wiki')));
  });
});
