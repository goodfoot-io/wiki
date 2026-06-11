/**
 * Tests that keybindings in the extension manifest are platform-compatible.
 *
 * VSCode does not automatically translate the `cmd` modifier to `ctrl` on
 * Windows or Linux -- a binding declared with `key: "shift+cmd+l"` is silently
 * ignored on those platforms. Any keybinding that uses `cmd` in its `key` field
 * (the default that applies when no platform-specific override is present) must
 * provide explicit `win` and `linux` overrides using platform-appropriate
 * modifiers such as `ctrl`.
 *
 * References:
 *   - https://code.visualstudio.com/api/references/extension-guidelines#keybindings
 *   - VSCode source: src/vs/platform/keybinding/common/keybindingLabels.ts
 *
 * @summary Keybinding platform-compatibility validation.
 * @module test/suite/keybindingPlatform.test
 */

import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as path from 'node:path';

interface KeybindingEntry {
  command: string;
  key?: string;
  mac?: string;
  win?: string;
  linux?: string;
}

interface ExtensionManifest {
  contributes?: {
    keybindings?: KeybindingEntry[];
  };
}

describe('keybinding platform compatibility', () => {
  const pkgPath = path.resolve(__dirname, '../../package.json');
  const pkg: ExtensionManifest = JSON.parse(fs.readFileSync(pkgPath, 'utf-8'));
  const bindings = pkg.contributes?.keybindings ?? [];

  it('should have at least one keybinding declared in package.json', () => {
    assert.ok(
      bindings.length > 0,
      'No keybindings array found in package.json contributes.keybindings. ' +
        'Cannot validate platform compatibility of an empty or missing keybindings section.'
    );
  });

  for (const binding of bindings) {
    const key = binding.key ?? '';
    const modifiers = key.split('+');
    const usesCmd = modifiers.includes('cmd');

    if (usesCmd) {
      const label = `keybinding "${binding.command}" (key="${key}")`;

      it(`${label} should have a win override to replace cmd with ctrl`, () => {
        assert.ok(
          binding.win != null,
          `Missing 'win' override. On Windows, the 'cmd' modifier does not exist -- ` +
            `VSCode silently drops bindings whose modifiers are unavailable on the current ` +
            `platform. Add: "win": "${key.replace('cmd', 'ctrl')}"`
        );
      });

      it(`${label} should have a linux override to replace cmd with ctrl`, () => {
        assert.ok(
          binding.linux != null,
          `Missing 'linux' override. On Linux, the 'cmd' modifier does not exist -- ` +
            `VSCode silently drops bindings whose modifiers are unavailable on the current ` +
            `platform. Add: "linux": "${key.replace('cmd', 'ctrl')}"`
        );
      });

      it(`${label} win override should not contain the cmd modifier`, () => {
        assert.ok(
          binding.win != null,
          'Cannot evaluate win override because it is missing (previous assertion covers this).'
        );
        if (binding.win != null) {
          const winModifiers = binding.win.split('+');
          assert.ok(
            !winModifiers.includes('cmd'),
            `'win' override "${binding.win}" still uses the 'cmd' modifier, which is invalid on Windows.`
          );
        }
      });

      it(`${label} linux override should not contain the cmd modifier`, () => {
        assert.ok(
          binding.linux != null,
          'Cannot evaluate linux override because it is missing (previous assertion covers this).'
        );
        if (binding.linux != null) {
          const linuxModifiers = binding.linux.split('+');
          assert.ok(
            !linuxModifiers.includes('cmd'),
            `'linux' override "${binding.linux}" still uses the 'cmd' modifier, which is invalid on Linux.`
          );
        }
      });
    }
  }
});
