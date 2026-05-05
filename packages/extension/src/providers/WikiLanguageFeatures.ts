/**
 * Editor language features for wiki markdown files: autocomplete, hover,
 * diagnostics on save, find references, and rename.
 *
 * Every feature derives the target namespace from the active document or from
 * an explicit `ns:` prefix in a qualified wikilink (`[[ns:Title]]`), then
 * shells out to the `wiki` CLI with `-n <ns>`.
 *
 * @summary Editor language features for wiki files.
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { getSourceArgs } from '../utils/sourceMode.js';
import { runWikiCommand } from '../utils/wikiBinary.js';
import type { WikiBinaryManager } from '../utils/wikiInstaller.js';
import type { NamespaceCache } from '../wiki/namespaceCache.js';
import { parseQualifiedWikilink } from '../wiki/wikilinkParser.js';

// ---------------------------------------------------------------------------
// CLI response types
// ---------------------------------------------------------------------------

/** Item returned by `wiki list --format json`. */
interface WikiListItem {
  title: string;
  aliases: string[];
  tags: string[];
  summary: string;
  file: string;
}

/** Item returned by `wiki links --format json`. */
interface WikiLinksResult {
  title: string;
  file: string;
  summary: string;
  snippets: Array<{ line: number; text: string }>;
}

/** Single check diagnostic from `wiki check --format json`. */
interface CheckDiag {
  kind: string;
  file: string;
  line: number;
  message: string;
}

/** Output of `wiki check --format json`. */
interface CheckOutput {
  errors: CheckDiag[];
}

/** Output of `wiki summary --format json`. */
interface WikiSummaryJson {
  title: string;
  file: string;
  summary?: string;
  aliases?: string[];
  tags?: string[];
}

// ---------------------------------------------------------------------------
// Provider class
// ---------------------------------------------------------------------------

/**
 * Registers and owns all wiki editor language feature providers.
 *
 * Call `register()` during extension activation and push the returned
 * disposables into `context.subscriptions`.
 */
export class WikiLanguageFeatures {
  private readonly _checkDiagnostics: vscode.DiagnosticCollection;
  private readonly _disposables: vscode.Disposable[] = [];

  constructor(
    private readonly _binaryManager: WikiBinaryManager,
    private readonly _namespaceCache: NamespaceCache
  ) {
    this._checkDiagnostics = vscode.languages.createDiagnosticCollection('wiki-check');
  }

  /**
   * Register all language feature providers.
   *
   * @returns Disposables that should be pushed into `context.subscriptions`
   *          by the caller.
   */
  register(): vscode.Disposable[] {
    const disposables: vscode.Disposable[] = [
      this._registerCompletionProvider(),
      this._registerHoverProvider(),
      this._registerDiagnosticsOnSave(),
      this._registerReferenceProvider(),
      this._registerRenameProvider(),
      this._checkDiagnostics
    ];
    this._disposables.push(...disposables);
    return disposables;
  }

  /** Dispose all registered providers and the diagnostic collection. */
  dispose(): void {
    for (const d of this._disposables) {
      d.dispose();
    }
    this._disposables.length = 0;
  }

  // --------------------------------------------------------------------------
  // Helpers
  // --------------------------------------------------------------------------

  /**
   * Return the workspace root filesystem path.
   *
   * @returns The first workspace folder path, or `undefined` when no folder
   *          is open.
   */
  private _workspaceRoot(): string | undefined {
    return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  }

  /**
   * Check whether `uri` belongs to a wiki known to the cache.
   *
   * @param uri - The file URI to test.
   * @returns `true` when the file is a `.wiki.md` file or an `.md` file
   *          inside a known wiki root.
   */
  private _isWikiFile(uri: vscode.Uri): boolean {
    if (uri.fsPath.endsWith('.wiki.md')) return true;
    if (!uri.fsPath.endsWith('.md')) return false;

    // Check against all known namespace roots.
    const all = this._namespaceCache.getAll();
    for (const ns of all) {
      if (uri.fsPath.startsWith(ns.absPath)) {
        return true;
      }
    }

    // When the namespace cache is still populating, fall back to a synchronous
    // parent-directory walk looking for wiki.toml. This catches peer-namespace
    // files (e.g. mesh/) that would otherwise be missed by the hardcoded wiki/ prefix.
    {
      let dir = path.dirname(uri.fsPath);
      const workspaceRoot = this._workspaceRoot();
      while (workspaceRoot != null && dir.startsWith(workspaceRoot)) {
        if (fs.existsSync(path.join(dir, 'wiki.toml'))) {
          return true;
        }
        if (dir === workspaceRoot) break;
        const parentDir = path.dirname(dir);
        if (parentDir === dir) break; // Reached filesystem root
        dir = parentDir;
      }
    }

    // Legacy fallback: default wiki/ directory.
    const wsRoot = this._workspaceRoot();
    if (wsRoot == null) return false;
    return uri.fsPath.startsWith(`${wsRoot}/wiki/`);
  }

  /**
   * Find a `[[wikilink]]` on the current line at `position`.
   *
   * @param document - The text document to scan.
   * @param position - The cursor position within the document.
   * @returns The inner content (between `[[` and `]]`), or `null` when the
   *          cursor is not inside a wikilink.
   */
  private _findWikilinkContentAtPosition(document: vscode.TextDocument, position: vscode.Position): string | null {
    const line = document.lineAt(position.line).text;
    const pos = position.character;

    // Keep the regex simple: match anything between [[ and ]].
    const regex = /\[\[([^\]]*)\]\]/g;
    let match: RegExpExecArray | null;
    for (match = regex.exec(line); match !== null; match = regex.exec(line)) {
      const start = match.index;
      const end = start + match[0].length;
      if (pos >= start && pos <= end) {
        return match[1]!;
      }
    }
    return null;
  }

  /**
   * Extract the typed wikilink prefix at the cursor position for completion.
   *
   * @param document - The text document to scan.
   * @param position - The cursor position.
   * @returns `null` when the cursor is not inside a `[[...` context.
   *          On success returns `{ ns, filterText, startChar }` where
   *          `filterText` is the text after `[[` (or after `ns:`) that the
   *          user has typed, and `startChar` is the line-column from which
   *          the replacement range should begin.
   */
  private _findWikilinkPrefixForCompletion(
    document: vscode.TextDocument,
    position: vscode.Position
  ): { ns: string; filterText: string; startChar: number } | null {
    const line = document.lineAt(position.line).text;
    const pos = position.character;
    const textBeforeCursor = line.substring(0, pos);

    // Find the last [[ before the cursor.
    const lastOpen = textBeforeCursor.lastIndexOf('[[');
    if (lastOpen === -1) return null;

    const typedBetween = textBeforeCursor.substring(lastOpen + 2);

    // If the wikilink is already closed, do not provide completions.
    if (typedBetween.includes(']]')) return null;

    // Check for an explicit namespace prefix.
    const colonIdx = typedBetween.indexOf(':');
    if (colonIdx > 0) {
      // User typed [[ns:... — derive namespace and filter from typed prefix.
      const ns = typedBetween.substring(0, colonIdx);
      const filterText = typedBetween.substring(colonIdx + 1);
      return { ns, filterText, startChar: lastOpen + 2 + colonIdx + 1 };
    }

    // No namespace prefix: use the current document's namespace.
    const ns = this._namespaceCache.resolveNamespaceForFile(document.uri.fsPath) ?? 'default';
    return { ns, filterText: typedBetween, startChar: lastOpen + 2 };
  }

  /**
   * Run a wiki CLI command and return the parsed JSON result.
   *
   * @param args - CLI arguments (excluding the binary path).
   * @returns Parsed JSON output, or `null` on non-zero exit, empty stdout,
   *          or parse failure.
   */
  private async _runWikiJson<T>(args: string[]): Promise<T | null> {
    const wsRoot = this._workspaceRoot();
    try {
      const handle = await this._binaryManager.ready();
      const sourceArgs = getSourceArgs();
      const result = await runWikiCommand(handle.path, [...sourceArgs, ...args], undefined, wsRoot);
      if (result.exitCode !== 0 || result.stdout.trim() === '') {
        return null;
      }
      return JSON.parse(result.stdout) as T;
    } catch {
      return null;
    }
  }

  // --------------------------------------------------------------------------
  // 6a. Completion (autocomplete)
  // --------------------------------------------------------------------------

  private _registerCompletionProvider(): vscode.Disposable {
    return vscode.languages.registerCompletionItemProvider(
      [{ language: 'markdown' }, { pattern: '**/*.wiki.md' }],
      {
        provideCompletionItems: async (
          document: vscode.TextDocument,
          position: vscode.Position
        ): Promise<vscode.CompletionItem[] | undefined> => {
          if (!this._isWikiFile(document.uri)) return undefined;

          const info = this._findWikilinkPrefixForCompletion(document, position);
          if (info == null) return undefined;

          const items = await this._runWikiJson<WikiListItem[]>(['-n', info.ns, 'list', '--format', 'json']);
          if (items == null) return undefined;

          const range = new vscode.Range(position.line, info.startChar, position.line, position.character);

          return items.map((item) => {
            const ci = new vscode.CompletionItem(item.title, vscode.CompletionItemKind.Reference);
            ci.detail = item.summary;
            ci.documentation = new vscode.MarkdownString(`**File:** \`${item.file}\``);
            // When the user typed a namespace prefix, insert the full
            // qualified form so the wikilink resolves across namespaces.
            const colonIdx = info.filterText.length === 0 ? -1 : info.filterText.lastIndexOf(':');
            // We stored the effective ns. Insert qualified if the namespace
            // differs from the current document's namespace.
            const currentNs = this._namespaceCache.resolveNamespaceForFile(document.uri.fsPath) ?? 'default';
            if (info.ns !== currentNs && colonIdx < 0) {
              ci.insertText = `${info.ns}:${item.title}`;
            } else {
              ci.insertText = item.title;
            }
            ci.range = range;
            ci.sortText = item.title.toLowerCase();
            return ci;
          });
        }
      },
      '['
    );
  }

  // --------------------------------------------------------------------------
  // 6b. Hover
  // --------------------------------------------------------------------------

  private _registerHoverProvider(): vscode.Disposable {
    return vscode.languages.registerHoverProvider([{ language: 'markdown' }, { pattern: '**/*.wiki.md' }], {
      provideHover: async (
        document: vscode.TextDocument,
        position: vscode.Position
      ): Promise<vscode.Hover | undefined> => {
        if (!this._isWikiFile(document.uri)) return undefined;

        const wikilinkContent = this._findWikilinkContentAtPosition(document, position);
        if (wikilinkContent == null) return undefined;

        const parsed = parseQualifiedWikilink(wikilinkContent);

        // Resolve the namespace: explicit from qualified wikilink, or
        // inherit from the current document.
        const ns = parsed.namespace ?? this._namespaceCache.resolveNamespaceForFile(document.uri.fsPath) ?? 'default';

        const summary = await this._runWikiJson<WikiSummaryJson>([
          '-n',
          ns,
          'summary',
          parsed.title,
          '--format',
          'json'
        ]);
        if (summary == null) return undefined;

        const md = new vscode.MarkdownString();
        md.appendMarkdown(`**${summary.title}**`);
        if (parsed.namespace != null) {
          md.appendMarkdown(` \\[namespace: \`${parsed.namespace}\`\\]`);
        }
        if (summary.summary != null && summary.summary.length > 0) {
          md.appendMarkdown(`\n\n${summary.summary}`);
        }
        md.appendMarkdown(`\n\n_File: \`${summary.file}\`_`);

        return new vscode.Hover(md);
      }
    });
  }

  // --------------------------------------------------------------------------
  // 6c. Diagnostics on save
  // --------------------------------------------------------------------------

  private _registerDiagnosticsOnSave(): vscode.Disposable {
    return vscode.workspace.onDidSaveTextDocument(async (document: vscode.TextDocument) => {
      if (!this._isWikiFile(document.uri)) return;

      const wsRoot = this._workspaceRoot();
      if (wsRoot == null) return;

      const ns = this._namespaceCache.resolveNamespaceForFile(document.uri.fsPath);
      if (ns == null) {
        // File belongs to a wiki file (by extension) but no namespace
        // root owns it — clear any stale diagnostics.
        this._checkDiagnostics.delete(document.uri);
        return;
      }

      // Clear stale diagnostics for this file before checking.
      this._checkDiagnostics.delete(document.uri);

      const output = await this._runWikiJson<CheckOutput>(['-n', ns, 'check', '--format', 'json']);
      if (output == null) return;

      const diagnostics: vscode.Diagnostic[] = [];
      for (const err of output.errors) {
        // Only surface diagnostics for the saved file.
        if (err.file !== document.uri.fsPath) continue;

        // CLI line numbers are 1-based; VS Code is 0-based.
        const line = err.line > 0 ? err.line - 1 : 0;
        const range = new vscode.Range(line, 0, line, Number.MAX_SAFE_INTEGER);

        const diag = new vscode.Diagnostic(
          range,
          err.message,
          // Treat cross-namespace issues and broken wikilinks as errors;
          // alias_resolve is a warning.
          err.kind === 'alias_resolve' ? vscode.DiagnosticSeverity.Warning : vscode.DiagnosticSeverity.Error
        );
        diag.source = `wiki:${ns}`;
        diag.code = err.kind;
        diagnostics.push(diag);
      }

      this._checkDiagnostics.set(document.uri, diagnostics);
    });
  }

  // --------------------------------------------------------------------------
  // 6d. Find References
  // --------------------------------------------------------------------------

  private _registerReferenceProvider(): vscode.Disposable {
    return vscode.languages.registerReferenceProvider([{ language: 'markdown' }, { pattern: '**/*.wiki.md' }], {
      provideReferences: async (
        document: vscode.TextDocument,
        position: vscode.Position,
        _context: vscode.ReferenceContext,
        _token: vscode.CancellationToken
      ): Promise<vscode.Location[] | undefined> => {
        if (!this._isWikiFile(document.uri)) return undefined;

        const wikilinkContent = this._findWikilinkContentAtPosition(document, position);
        if (wikilinkContent == null) return undefined;

        const parsed = parseQualifiedWikilink(wikilinkContent);

        // Gather references from the target namespace first.
        const ns = parsed.namespace ?? this._namespaceCache.resolveNamespaceForFile(document.uri.fsPath) ?? 'default';

        const locations: vscode.Location[] = [];

        // Query the specific namespace.
        const nsResults = await this._collectLinkResults(ns, parsed.title);
        if (nsResults != null) {
          locations.push(...nsResults);
        }

        // Also query across all known namespaces for comprehensive results.
        const allNamespaces = this._namespaceCache.getAll();
        for (const nsInfo of allNamespaces) {
          if (nsInfo.namespace === ns) continue; // already queried
          const crossResults = await this._collectLinkResults(nsInfo.namespace, parsed.title);
          if (crossResults != null) {
            locations.push(...crossResults);
          }
        }

        return locations.length > 0 ? locations : undefined;
      }
    });
  }

  /**
   * Query `wiki -n <ns> links <target> --format json` and convert results to
   * VS Code Locations.
   *
   * @param ns     - Namespace to scope the links query.
   * @param target - Page title to search for incoming links.
   * @returns Array of VS Code Locations, or `null` when no links are found.
   */
  private async _collectLinkResults(ns: string, target: string): Promise<vscode.Location[] | null> {
    const results = await this._runWikiJson<WikiLinksResult[]>(['-n', ns, 'links', target, '--format', 'json']);
    if (results == null || results.length === 0) return null;

    const locations: vscode.Location[] = [];
    for (const r of results) {
      const uri = vscode.Uri.file(r.file);
      // Use the first snippet's line number; CLI lines are 1-based.
      if (r.snippets.length > 0) {
        const line = r.snippets[0]!.line;
        const range = new vscode.Range(line - 1, 0, line - 1, Number.MAX_SAFE_INTEGER);
        locations.push(new vscode.Location(uri, range));
      } else {
        // Fallback: reference the start of the file.
        locations.push(new vscode.Location(uri, new vscode.Position(0, 0)));
      }
    }
    return locations;
  }

  // --------------------------------------------------------------------------
  // 6e. Rename
  // --------------------------------------------------------------------------

  private _registerRenameProvider(): vscode.Disposable {
    return vscode.languages.registerRenameProvider([{ language: 'markdown' }, { pattern: '**/*.wiki.md' }], {
      prepareRename: (document: vscode.TextDocument, position: vscode.Position): vscode.Range | undefined => {
        if (!this._isWikiFile(document.uri)) return undefined;

        const wikilinkContent = this._findWikilinkContentAtPosition(document, position);
        if (wikilinkContent == null) return undefined;

        // Return the range of the full [[...]] so VS Code highlights it.
        const line = document.lineAt(position.line).text;
        const regex = /\[\[([^\]]*)\]\]/g;
        let match: RegExpExecArray | null;
        for (match = regex.exec(line); match !== null; match = regex.exec(line)) {
          if (position.character >= match.index && position.character <= match.index + match[0].length) {
            return new vscode.Range(position.line, match.index, position.line, match.index + match[0].length);
          }
        }
        return undefined;
      },

      provideRenameEdits: async (
        document: vscode.TextDocument,
        position: vscode.Position,
        newName: string,
        _token: vscode.CancellationToken
      ): Promise<vscode.WorkspaceEdit | undefined> => {
        if (!this._isWikiFile(document.uri)) return undefined;

        const wikilinkContent = this._findWikilinkContentAtPosition(document, position);
        if (wikilinkContent == null) return undefined;

        const parsed = parseQualifiedWikilink(wikilinkContent);
        const oldTitle = parsed.title;

        // Resolve the target namespace.
        const ns = parsed.namespace ?? this._namespaceCache.resolveNamespaceForFile(document.uri.fsPath) ?? 'default';

        // Collect all references (same approach as find-references).
        const allResults: Array<{ file: string; snippets: Array<{ line: number; text: string }> }> = [];

        const nsResults = await this._runWikiJson<WikiLinksResult[]>(['-n', ns, 'links', oldTitle, '--format', 'json']);
        if (nsResults != null) {
          allResults.push(...nsResults);
        }

        // Cross-namespace references.
        const allNamespaces = this._namespaceCache.getAll();
        for (const nsInfo of allNamespaces) {
          if (nsInfo.namespace === ns) continue;
          const crossResults = await this._runWikiJson<WikiLinksResult[]>([
            '-n',
            nsInfo.namespace,
            'links',
            oldTitle,
            '--format',
            'json'
          ]);
          if (crossResults != null) {
            allResults.push(...crossResults);
          }
        }

        if (allResults.length === 0) return new vscode.WorkspaceEdit();

        const edit = new vscode.WorkspaceEdit();

        for (const loc of allResults) {
          const uri = vscode.Uri.file(loc.file);
          const doc = await vscode.workspace.openTextDocument(uri);

          for (const snippet of loc.snippets) {
            const lineIdx = snippet.line - 1; // CLI is 1-based
            if (lineIdx < 0 || lineIdx >= doc.lineCount) continue;

            const lineText = doc.lineAt(lineIdx).text;

            // Find every wikilink on this line that references oldTitle.
            const wikilinkRegex = /\[\[([^\]]*)\]\]/g;
            let wlMatch: RegExpExecArray | null;
            for (wlMatch = wikilinkRegex.exec(lineText); wlMatch !== null; wlMatch = wikilinkRegex.exec(lineText)) {
              const content = wlMatch[1]!;
              let parsedContent: ReturnType<typeof parseQualifiedWikilink>;
              try {
                parsedContent = parseQualifiedWikilink(content);
              } catch {
                continue;
              }

              if (parsedContent.title !== oldTitle) continue;

              // Calculate the range of the title within this wikilink.
              // The title starts after the optional "ns:" prefix.
              const colonIdx = content.indexOf(':');
              const titleStartInContent = colonIdx >= 0 ? colonIdx + 1 : 0;

              // Offset: 2 for the [[ prefix + title start position in content.
              const titleStartChar = wlMatch.index + 2 + titleStartInContent;
              const titleEndChar = titleStartChar + oldTitle.length;

              const range = new vscode.Range(lineIdx, titleStartChar, lineIdx, titleEndChar);
              edit.replace(uri, range, newName);
            }
          }
        }

        return edit;
      }
    });
  }
}
