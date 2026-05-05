/**
 * Scaffold command that discovers mesh definitions from wiki fragment links
 * across all wiki roots.
 *
 * Calls `wiki scaffold --format json` without a namespace flag (the scaffold
 * command operates across all wikis by design). Reports parse errors as VS
 * Code diagnostics and surfaces mesh definitions in an output channel.
 *
 * @summary Scaffold command for mesh discovery from fragment links.
 */

import * as vscode from 'vscode';
import { runWikiCommand } from '../utils/wikiBinary.js';
import type { WikiBinaryManager } from '../utils/wikiInstaller.js';

// ---------------------------------------------------------------------------
// Types matching `wiki scaffold --format json` output
// ---------------------------------------------------------------------------

interface ScaffoldAnchor {
  path: string;
  startLine: number;
  endLine: number;
}

interface ScaffoldMesh {
  slug: string;
  headingChain: string;
  sectionOpening: string;
  anchors: ScaffoldAnchor[];
}

interface ScaffoldPage {
  title: string;
  file: string;
  meshes: ScaffoldMesh[];
}

interface ScaffoldOutput {
  schemaVersion: number;
  parseErrors: Array<{ file: string; line: number; message: string }>;
  pages: ScaffoldPage[];
}

// ---------------------------------------------------------------------------
// Persistent resources (reused across command invocations)
// ---------------------------------------------------------------------------

/** Diagnostic collection for scaffold parse errors. */
let _diagnosticsCollection: vscode.DiagnosticCollection | undefined;

/** Output channel for scaffold mesh definitions. */
let _outputChannel: vscode.OutputChannel | undefined;

function getDiagnosticsCollection(): vscode.DiagnosticCollection {
  if (_diagnosticsCollection == null) {
    _diagnosticsCollection = vscode.languages.createDiagnosticCollection('wiki-scaffold');
  }
  return _diagnosticsCollection;
}

function getOutputChannel(): vscode.OutputChannel {
  if (_outputChannel == null) {
    _outputChannel = vscode.window.createOutputChannel('Wiki Scaffold');
  }
  return _outputChannel;
}

// ---------------------------------------------------------------------------
// Command handler
// ---------------------------------------------------------------------------

/**
 * Run the scaffold command and surface results.
 *
 * @param binaryManager - Service that resolves or installs the wiki CLI.
 */
export async function wikiScaffold(binaryManager: WikiBinaryManager): Promise<void> {
  let binaryPath: string;
  try {
    binaryPath = (
      await vscode.window.withProgress(
        { location: vscode.ProgressLocation.Notification, title: 'Scaffolding wiki meshes…' },
        () => binaryManager.ready()
      )
    ).path;
  } catch (error) {
    void vscode.window.showErrorMessage(`Wiki: ${binaryManager.formatFailure(error)}`);
    return;
  }

  const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;

  // Run scaffold across all wiki roots (no -n flag — operates globally).
  const result = await runWikiCommand(binaryPath, ['scaffold', '--format', 'json'], undefined, workspaceRoot);
  if (result.exitCode !== 0) {
    void vscode.window.showErrorMessage(
      `Wiki scaffold failed: ${result.stderr.trim() || `exit code ${result.exitCode}`}`
    );
    return;
  }

  let output: ScaffoldOutput;
  try {
    output = JSON.parse(result.stdout) as ScaffoldOutput;
  } catch (parseErr) {
    void vscode.window.showErrorMessage(
      `Wiki scaffold: failed to parse output: ${parseErr instanceof Error ? parseErr.message : String(parseErr)}`
    );
    return;
  }

  // -----------------------------------------------------------------------
  // Report parse errors as diagnostics
  // -----------------------------------------------------------------------
  const diagnosticsCollection = getDiagnosticsCollection();
  diagnosticsCollection.clear();

  if (output.parseErrors.length > 0) {
    const diagMap = new Map<string, vscode.Diagnostic[]>();
    for (const err of output.parseErrors) {
      // CLI line numbers are 1-based; VS Code is 0-based.
      const line = err.line > 0 ? err.line - 1 : 0;
      const range = new vscode.Range(line, 0, line, Number.MAX_SAFE_INTEGER);
      const diag = new vscode.Diagnostic(range, err.message, vscode.DiagnosticSeverity.Error);
      diag.source = 'wiki-scaffold';
      const existing = diagMap.get(err.file) ?? [];
      existing.push(diag);
      diagMap.set(err.file, existing);
    }
    for (const [filePath, diags] of diagMap) {
      diagnosticsCollection.set(vscode.Uri.file(filePath), diags);
    }
  }

  // -----------------------------------------------------------------------
  // Count meshes
  // -----------------------------------------------------------------------
  let meshCount = 0;
  for (const page of output.pages) {
    meshCount += page.meshes.length;
  }

  // -----------------------------------------------------------------------
  // Show informational message
  // -----------------------------------------------------------------------
  void vscode.window.showInformationMessage(`Scaffolded ${meshCount} meshes from ${output.pages.length} pages`);

  // -----------------------------------------------------------------------
  // Surface mesh definitions in the output channel
  // -----------------------------------------------------------------------
  const channel = getOutputChannel();
  channel.clear();
  channel.appendLine('Wiki Scaffold Results');
  channel.appendLine('');

  for (const page of output.pages) {
    if (page.meshes.length === 0) continue;
    channel.appendLine(`=== ${page.title} (${page.file}) ===`);
    for (const mesh of page.meshes) {
      channel.appendLine('');
      channel.appendLine(`  Mesh: ${mesh.slug}`);
      channel.appendLine(`    Heading Chain: ${mesh.headingChain}`);
      channel.appendLine(`    Section Opening: ${mesh.sectionOpening}`);
      channel.appendLine('    Anchors:');
      for (const anchor of mesh.anchors) {
        channel.appendLine(`      - ${anchor.path}#L${anchor.startLine}-L${anchor.endLine}`);
      }
    }
    channel.appendLine('');
  }

  channel.show();
}
