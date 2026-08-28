import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');

interface VersionedDoc {
  version?: unknown;
}

function readVersion(relativePath: string): string {
  const doc = JSON.parse(readFileSync(resolve(repoRoot, relativePath), 'utf8')) as VersionedDoc;
  expect(doc.version, `${relativePath} carries no version`).toEqual(expect.any(String));
  return doc.version as string;
}

describe('version consistency across the four version-bearing surfaces', () => {
  it('claude, codex, opencode, and antigravity manifests and the marketplace entry agree', () => {
    const claudeManifest = readVersion('plugins-claude/wiki/.claude-plugin/plugin.json');
    const codexManifest = readVersion('plugins-codex/wiki/.codex-plugin/plugin.json');
    const opencodePackage = readVersion('plugins-opencode/wiki/package.json');
    const antigravityManifest = readVersion('plugins-antigravity/wiki/plugin.json');

    const marketplacePath = '.claude-plugin/marketplace.json';
    const marketplace = JSON.parse(readFileSync(resolve(repoRoot, marketplacePath), 'utf8')) as {
      plugins?: VersionedDoc[];
    };
    const marketplaceEntry = marketplace.plugins?.[0]?.version;
    expect(marketplaceEntry, `${marketplacePath} plugins[0] carries no version`).toEqual(expect.any(String));

    const surfaces: Record<string, string> = {
      'plugins-claude manifest': claudeManifest,
      'plugins-codex manifest': codexManifest,
      'plugins-opencode package': opencodePackage,
      'plugins-antigravity manifest': antigravityManifest,
      'claude marketplace entry': marketplaceEntry as string
    };
    expect(new Set(Object.values(surfaces)).size, `version drift across surfaces: ${JSON.stringify(surfaces)}`).toBe(1);
  });
});
