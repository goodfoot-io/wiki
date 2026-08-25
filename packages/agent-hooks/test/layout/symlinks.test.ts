import { existsSync, lstatSync, realpathSync } from 'node:fs';
import { dirname, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { isWikiFile } from '../../src/common/wiki-check.js';

const repoRoot = realpathSync(resolve(dirname(fileURLToPath(import.meta.url)), '../../../..'));

const pluginSkillLinks = [
  'plugins-claude/wiki/skills/wiki',
  'plugins-codex/wiki/skills/wiki',
  'plugins-opencode/wiki/skills/wiki'
];

function assertResolvesInsideRepoToRealWikiSkill(linkPath: string): void {
  const link = resolve(repoRoot, linkPath);
  expect(lstatSync(link).isSymbolicLink(), `${linkPath} must be a symlink`).toBe(true);

  const target = realpathSync(link);
  expect(target.startsWith(repoRoot + sep), `${linkPath} must resolve inside the repo, got ${target}`).toBe(true);

  const skill = resolve(target, 'SKILL.md');
  expect(existsSync(skill), `${linkPath}/SKILL.md must exist`).toBe(true);

  expect(isWikiFile(skill, repoRoot), `${linkPath}/SKILL.md must carry non-empty frontmatter title+summary`).toBe(true);
}

describe('plugin tree skills symlinks', () => {
  for (const linkPath of pluginSkillLinks) {
    it(`${linkPath} resolves inside the repo to a real wiki skill`, () => {
      assertResolvesInsideRepoToRealWikiSkill(linkPath);
    });
  }

  it('.agents/skills/wiki resolves inside the repo', () => {
    assertResolvesInsideRepoToRealWikiSkill('.agents/skills/wiki');
  });
});
