/**
 * Production build script for the wiki-extension.
 *
 * Invoked via the `vscode:prepublish` lifecycle hook before `vsce package`.
 * Outputs into dist/ relative to the extension root so package.json's
 * `"main": "./dist/bundle.cjs"` resolves correctly.
 *
 * Outputs:
 *   dist/bundle.cjs        — extension host (CJS, vscode external)
 *   dist/wiki.js           — webview bundle (IIFE)
 *   dist/codicons/         — codicon.css + codicon.ttf
 */

import * as esbuild from 'esbuild';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';

const EXTENSION_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const DIST = path.join(EXTENSION_ROOT, 'dist');

fs.mkdirSync(DIST, { recursive: true });

// Extension host — runs in Node.js inside VS Code.
await esbuild.build({
  entryPoints: [path.join(EXTENSION_ROOT, 'src/extension.ts')],
  bundle: true,
  outfile: path.join(DIST, 'bundle.cjs'),
  format: 'cjs',
  platform: 'node',
  target: 'node22',
  external: ['vscode'],
  sourcemap: true,
  minify: true
});

// Webview — runs in the browser sandbox inside the panel.
await esbuild.build({
  entryPoints: [path.join(EXTENSION_ROOT, 'src/webviews/wiki/index.ts')],
  bundle: true,
  outfile: path.join(DIST, 'wiki.js'),
  format: 'iife',
  platform: 'browser',
  target: 'es2022',
  sourcemap: true,
  minify: true
});

// Codicon assets referenced by the webview CSP.
const codiconsOut = path.join(DIST, 'codicons');
fs.mkdirSync(codiconsOut, { recursive: true });
const codiconsSrc = path.dirname(fileURLToPath(import.meta.resolve('@vscode/codicons/dist/codicon.css')));
for (const file of ['codicon.css', 'codicon.ttf']) {
  fs.copyFileSync(path.join(codiconsSrc, file), path.join(codiconsOut, file));
}

console.log('[build-production] Done.');
