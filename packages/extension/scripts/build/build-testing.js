/**
 * Testing build script for the wiki-extension.
 *
 * Invoked by test/runTest.ts with TEST_DIST_DIR set to a unique temp path.
 * VS Code loads the extension from TEST_DIST_DIR as its root, using the
 * package.json written here (main: "./bundle.cjs"). The webview script is
 * at TEST_DIST_DIR/dist/wiki.js, matching _buildShellHtml's extensionUri paths.
 *
 * Outputs into TEST_DIST_DIR:
 *   package.json                    — extension manifest (main: ./bundle.cjs)
 *   bundle.cjs                      — extension host (CJS, vscode external)
 *   dist/wiki.js                    — webview bundle (IIFE)
 *   dist/codicons/                  — codicon.css + codicon.ttf
 *   media/                          — markdown.css, highlight.css
 *   test/suite/index.cjs            — Mocha runner entry point
 *   test/suite/*.test.cjs           — individual test suites
 */

import * as esbuild from 'esbuild';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { glob } from 'glob';

const EXTENSION_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const OUT_DIR = process.env['TEST_DIST_DIR'];

if (!OUT_DIR) {
  console.error('[build-testing] TEST_DIST_DIR env var is required.');
  process.exit(1);
}

fs.mkdirSync(OUT_DIR, { recursive: true });

// Write extension manifest. main points at bundle.cjs in the same directory.
const pkg = JSON.parse(fs.readFileSync(path.join(EXTENSION_ROOT, 'package.json'), 'utf-8'));
pkg.main = './bundle.cjs';
fs.writeFileSync(path.join(OUT_DIR, 'package.json'), JSON.stringify(pkg, null, 2));

// Extension host — runs in Node.js inside VS Code.
await esbuild.build({
  entryPoints: [path.join(EXTENSION_ROOT, 'src/extension.ts')],
  bundle: true,
  outfile: path.join(OUT_DIR, 'bundle.cjs'),
  format: 'cjs',
  platform: 'node',
  target: 'node22',
  external: ['vscode'],
  sourcemap: true
});

// Webview — runs in the browser sandbox inside the panel.
// extensionUri points at OUT_DIR, so the webview must be at OUT_DIR/dist/wiki.js.
const distDir = path.join(OUT_DIR, 'dist');
fs.mkdirSync(distDir, { recursive: true });
await esbuild.build({
  entryPoints: [path.join(EXTENSION_ROOT, 'src/webviews/wiki/index.ts')],
  bundle: true,
  outfile: path.join(distDir, 'wiki.js'),
  format: 'iife',
  platform: 'browser',
  target: 'es2022',
  sourcemap: true
});

// Codicon assets referenced by the webview CSP.
const codiconsOut = path.join(distDir, 'codicons');
fs.mkdirSync(codiconsOut, { recursive: true });
const codiconsSrc = path.dirname(fileURLToPath(import.meta.resolve('@vscode/codicons/dist/codicon.css')));
for (const file of ['codicon.css', 'codicon.ttf']) {
  fs.copyFileSync(path.join(codiconsSrc, file), path.join(codiconsOut, file));
}

// Media assets (CSS files) — referenced relative to extensionUri/media/.
const mediaOut = path.join(OUT_DIR, 'media');
fs.mkdirSync(mediaOut, { recursive: true });
const mediaSrc = path.join(EXTENSION_ROOT, 'media');
for (const file of fs.readdirSync(mediaSrc)) {
  fs.copyFileSync(path.join(mediaSrc, file), path.join(mediaOut, file));
}

// Test suite — Mocha runner entry point.
const testSuiteOut = path.join(OUT_DIR, 'test', 'suite');
fs.mkdirSync(testSuiteOut, { recursive: true });

await esbuild.build({
  entryPoints: [path.join(EXTENSION_ROOT, 'test/suite/index.ts')],
  bundle: true,
  outfile: path.join(testSuiteOut, 'index.cjs'),
  format: 'cjs',
  platform: 'node',
  target: 'node22',
  external: ['vscode', 'mocha'],
  sourcemap: true
});

// Individual test suites.
const testFiles = await glob('test/suite/**/*.test.ts', { cwd: EXTENSION_ROOT });
for (const rel of testFiles) {
  const baseName = path.basename(rel, '.ts') + '.cjs';
  await esbuild.build({
    entryPoints: [path.join(EXTENSION_ROOT, rel)],
    bundle: true,
    outfile: path.join(testSuiteOut, baseName),
    format: 'cjs',
    platform: 'node',
    target: 'node22',
    external: ['vscode', 'mocha'],
    sourcemap: true
  });
}

console.log('[build-testing] Done. Output:', OUT_DIR);
