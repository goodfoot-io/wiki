/**
 * Editor language features for wiki markdown files: autocomplete, hover,
 * diagnostics on save, find references, and rename.
 *
 * All features operate on filesystem paths. Markdown link targets are
 * resolved relative to the linking file's directory (standard markdown
 * semantics). Frontmatter `title` + `summary` are read directly from disk
 * to provide wiki-aware affordances (summary preview, ranked completion).
 *
 * @summary Editor language features for wiki files.
 */

import { readFile } from 'node:fs/promises';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { getSourceArgs } from '../utils/sourceMode.js';
import { runWikiCommand } from '../utils/wikiBinary.js';
import type { WikiBinaryManager } from '../utils/wikiInstaller.js';

/** Single diagnostic from `wiki check --format json`. */
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

/** Frontmatter fields used for wiki-aware affordances. */
interface FrontmatterInfo {
  title?: string;
  summary?: string;
}

/**
 * Match a standard markdown link `[label](href)` on a single line. The
 * regex skips images (`![...](...)`).
 */
const MARKDOWN_LINK_RE = /(?<!!)\[([^\]]*)\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g;

/**
 * Parse `---\nkey: value\n---` YAML frontmatter to extract title/summary.
 * Only string scalars are supported; quoted forms are unwrapped.
 *
 * @param text - Raw file contents.
 * @returns Parsed frontmatter info (empty when no frontmatter is present).
 */
function parseFrontmatter(text: string): FrontmatterInfo {
  if (!text.startsWith('---\n')) return {};
  const end = text.indexOf('\n---', 4);
  if (end < 0) return {};
  const block = text.slice(4, end);
  const info: FrontmatterInfo = {};
  for (const line of block.split('\n')) {
    const m = line.match(/^([A-Za-z_][A-Za-z0-9_-]*)\s*:\s*(.*)$/);
    if (m == null) continue;
    const key = m[1]!;
    let value = m[2]!.trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(1, -1);
    }
    if (key === 'title') info.title = value;
    if (key === 'summary') info.summary = value;
  }
  return info;
}

async function readFrontmatter(absPath: string): Promise<FrontmatterInfo | null> {
  try {
    const text = await readFile(absPath, 'utf8');
    return parseFrontmatter(text);
  } catch {
    return null;
  }
}

/**
 * Resolve a markdown link href to an absolute filesystem path, relative to
 * `fromFile`'s directory. Returns null for non-internal targets (http(s)/mailto/
 * fragment-only).
 *
 * @param href     - Raw markdown link target.
 * @param fromFile - Absolute path to the linking file.
 * @returns The absolute target path, or null when the link is external/empty.
 */
function resolveLinkTarget(href: string, fromFile: string): string | null {
  if (href === '' || href.startsWith('#')) return null;
  if (/^[a-z][a-z0-9+.-]*:/i.test(href)) return null;
  const hashIdx = href.indexOf('#');
  const rawPath = hashIdx >= 0 ? href.slice(0, hashIdx) : href;
  if (rawPath === '') return null;
  if (path.isAbsolute(rawPath)) return path.normalize(rawPath);
  return path.normalize(path.resolve(path.dirname(fromFile), rawPath));
}

export class WikiLanguageFeatures {
  private readonly _checkDiagnostics: vscode.DiagnosticCollection;
  private readonly _disposables: vscode.Disposable[] = [];

  constructor(private readonly _binaryManager: WikiBinaryManager) {
    this._checkDiagnostics = vscode.languages.createDiagnosticCollection('wiki-check');
  }

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

  dispose(): void {
    for (const d of this._disposables) {
      d.dispose();
    }
    this._disposables.length = 0;
  }

  // --------------------------------------------------------------------------
  // Helpers
  // --------------------------------------------------------------------------

  private _workspaceRoot(): string | undefined {
    return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  }

  /**
   * Check whether `uri` is a markdown file inside the active workspace.
   *
   * @param uri - The file URI to test.
   * @returns True when the file is a `.md` file under an open workspace folder.
   */
  private _isMarkdownFile(uri: vscode.Uri): boolean {
    if (!uri.fsPath.endsWith('.md')) return false;
    const wsRoot = this._workspaceRoot();
    if (wsRoot == null) return false;
    return uri.fsPath.startsWith(wsRoot + path.sep) || uri.fsPath === wsRoot;
  }

  /**
   * Find a markdown link at `position` and return its href plus the inner
   * range of the href.
   *
   * @param document - The text document to scan.
   * @param position - The cursor position within the document.
   * @returns The href and its line range, or null when no link is at the cursor.
   */
  private _findMarkdownLinkAtPosition(
    document: vscode.TextDocument,
    position: vscode.Position
  ): { href: string; hrefRange: vscode.Range } | null {
    const line = document.lineAt(position.line).text;
    const pos = position.character;
    const re = new RegExp(MARKDOWN_LINK_RE.source, 'g');
    let match: RegExpExecArray | null;
    for (match = re.exec(line); match !== null; match = re.exec(line)) {
      const start = match.index;
      const end = start + match[0].length;
      if (pos >= start && pos <= end) {
        const href = match[2]!;
        const hrefStart = line.indexOf(href, start);
        if (hrefStart < 0) continue;
        return {
          href,
          hrefRange: new vscode.Range(position.line, hrefStart, position.line, hrefStart + href.length)
        };
      }
    }
    return null;
  }

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

  /**
   * Find every `.md` file inside the open workspace.
   *
   * @returns URIs of every workspace markdown file (excluding `node_modules`).
   */
  private async _allMarkdownFiles(): Promise<vscode.Uri[]> {
    return vscode.workspace.findFiles('**/*.md', '**/node_modules/**');
  }

  // --------------------------------------------------------------------------
  // Completion
  // --------------------------------------------------------------------------

  private _registerCompletionProvider(): vscode.Disposable {
    return vscode.languages.registerCompletionItemProvider(
      [{ language: 'markdown' }],
      {
        provideCompletionItems: async (
          document: vscode.TextDocument,
          position: vscode.Position
        ): Promise<vscode.CompletionItem[] | undefined> => {
          if (!this._isMarkdownFile(document.uri)) return undefined;

          // Completion fires inside a markdown link href: `[label](|)`.
          const lineText = document.lineAt(position.line).text;
          const textBeforeCursor = lineText.substring(0, position.character);
          const openIdx = textBeforeCursor.lastIndexOf('](');
          if (openIdx < 0) return undefined;
          const between = textBeforeCursor.substring(openIdx + 2);
          if (between.includes(')') || between.includes(' ')) return undefined;

          const sourceDir = path.dirname(document.uri.fsPath);
          const files = await this._allMarkdownFiles();

          const items: vscode.CompletionItem[] = [];
          for (const fileUri of files) {
            const relPath = path.relative(sourceDir, fileUri.fsPath);
            if (relPath === '') continue;
            // Normalise separators to POSIX for markdown links.
            const href = relPath.split(path.sep).join('/');

            const ci = new vscode.CompletionItem(href, vscode.CompletionItemKind.File);
            ci.insertText = href;

            const fm = await readFrontmatter(fileUri.fsPath);
            if (fm?.title != null && fm.summary != null) {
              ci.detail = fm.title;
              ci.documentation = new vscode.MarkdownString(fm.summary);
              // Sort wiki-aware files (with title + summary) before plain
              // markdown files.
              ci.sortText = `0_${fm.title.toLowerCase()}`;
            } else {
              ci.sortText = `1_${href.toLowerCase()}`;
            }

            items.push(ci);
          }
          return items;
        }
      },
      '('
    );
  }

  // --------------------------------------------------------------------------
  // Hover
  // --------------------------------------------------------------------------

  private _registerHoverProvider(): vscode.Disposable {
    return vscode.languages.registerHoverProvider([{ language: 'markdown' }], {
      provideHover: async (
        document: vscode.TextDocument,
        position: vscode.Position
      ): Promise<vscode.Hover | undefined> => {
        if (!this._isMarkdownFile(document.uri)) return undefined;

        const link = this._findMarkdownLinkAtPosition(document, position);
        if (link == null) return undefined;

        const absTarget = resolveLinkTarget(link.href, document.uri.fsPath);
        if (absTarget == null) return undefined;

        const fm = await readFrontmatter(absTarget);
        const md = new vscode.MarkdownString();
        const wsRoot = this._workspaceRoot();
        const relForDisplay =
          wsRoot != null && absTarget.startsWith(wsRoot + path.sep) ? absTarget.slice(wsRoot.length + 1) : absTarget;

        if (fm?.summary != null) {
          if (fm.title != null) md.appendMarkdown(`**${fm.title}**\n\n`);
          md.appendMarkdown(`${fm.summary}\n\n`);
          md.appendMarkdown(`_File: \`${relForDisplay}\`_`);
        } else {
          md.appendMarkdown(`\`${relForDisplay}\``);
        }
        return new vscode.Hover(md, link.hrefRange);
      }
    });
  }

  // --------------------------------------------------------------------------
  // Diagnostics on save
  // --------------------------------------------------------------------------

  private _registerDiagnosticsOnSave(): vscode.Disposable {
    return vscode.workspace.onDidSaveTextDocument(async (document: vscode.TextDocument) => {
      if (!this._isMarkdownFile(document.uri)) return;

      this._checkDiagnostics.delete(document.uri);

      const output = await this._runWikiJson<CheckOutput>(['check', '--format', 'json']);
      if (output == null) return;

      const diagnostics: vscode.Diagnostic[] = [];
      for (const err of output.errors) {
        if (err.file !== document.uri.fsPath) continue;

        const line = err.line > 0 ? err.line - 1 : 0;
        const range = new vscode.Range(line, 0, line, Number.MAX_SAFE_INTEGER);

        const diag = new vscode.Diagnostic(range, err.message, vscode.DiagnosticSeverity.Error);
        diag.source = 'wiki';
        diag.code = err.kind;
        diagnostics.push(diag);
      }

      this._checkDiagnostics.set(document.uri, diagnostics);
    });
  }

  // --------------------------------------------------------------------------
  // Find References
  // --------------------------------------------------------------------------

  private _registerReferenceProvider(): vscode.Disposable {
    return vscode.languages.registerReferenceProvider([{ language: 'markdown' }], {
      provideReferences: async (
        document: vscode.TextDocument,
        position: vscode.Position,
        _context: vscode.ReferenceContext,
        _token: vscode.CancellationToken
      ): Promise<vscode.Location[] | undefined> => {
        if (!this._isMarkdownFile(document.uri)) return undefined;

        const link = this._findMarkdownLinkAtPosition(document, position);
        const targetPath = link != null ? resolveLinkTarget(link.href, document.uri.fsPath) : document.uri.fsPath;
        if (targetPath == null) return undefined;

        return this._findIncomingLinks(targetPath);
      }
    });
  }

  /**
   * Scan every markdown file in the workspace for a link whose resolved
   * absolute path equals `targetAbsPath`.
   *
   * @param targetAbsPath - Absolute path to the file being referenced.
   * @returns Locations of every matching link.
   */
  private async _findIncomingLinks(targetAbsPath: string): Promise<vscode.Location[]> {
    const files = await this._allMarkdownFiles();
    const locations: vscode.Location[] = [];

    for (const fileUri of files) {
      let text: string;
      try {
        text = await readFile(fileUri.fsPath, 'utf8');
      } catch {
        continue;
      }

      const lines = text.split('\n');
      for (let lineIdx = 0; lineIdx < lines.length; lineIdx++) {
        const lineText = lines[lineIdx]!;
        const re = new RegExp(MARKDOWN_LINK_RE.source, 'g');
        let match: RegExpExecArray | null;
        for (match = re.exec(lineText); match !== null; match = re.exec(lineText)) {
          const href = match[2]!;
          const resolved = resolveLinkTarget(href, fileUri.fsPath);
          if (resolved !== targetAbsPath) continue;
          const hrefStart = lineText.indexOf(href, match.index);
          if (hrefStart < 0) continue;
          locations.push(
            new vscode.Location(fileUri, new vscode.Range(lineIdx, hrefStart, lineIdx, hrefStart + href.length))
          );
        }
      }
    }

    return locations;
  }

  // --------------------------------------------------------------------------
  // Rename (file move)
  // --------------------------------------------------------------------------

  /**
   * Build a WorkspaceEdit that rewrites every markdown link whose resolved
   * absolute target equals `oldAbsPath` so that its href becomes a relative
   * path from the linking file's directory to `newAbsPath`. Used by file-
   * move handlers and by the rename provider.
   *
   * @param oldAbsPath - Absolute path the link previously resolved to.
   * @param newAbsPath - Absolute path the link should now resolve to.
   * @returns A WorkspaceEdit replacing every matching href.
   */
  async buildFileMoveEdit(oldAbsPath: string, newAbsPath: string): Promise<vscode.WorkspaceEdit> {
    const edit = new vscode.WorkspaceEdit();
    const files = await this._allMarkdownFiles();

    for (const fileUri of files) {
      let text: string;
      try {
        text = await readFile(fileUri.fsPath, 'utf8');
      } catch {
        continue;
      }

      const lines = text.split('\n');
      for (let lineIdx = 0; lineIdx < lines.length; lineIdx++) {
        const lineText = lines[lineIdx]!;
        const re = new RegExp(MARKDOWN_LINK_RE.source, 'g');
        let match: RegExpExecArray | null;
        for (match = re.exec(lineText); match !== null; match = re.exec(lineText)) {
          const href = match[2]!;
          const hashIdx = href.indexOf('#');
          const rawPath = hashIdx >= 0 ? href.slice(0, hashIdx) : href;
          const fragment = hashIdx >= 0 ? href.slice(hashIdx) : '';
          const resolved = resolveLinkTarget(rawPath, fileUri.fsPath);
          if (resolved !== oldAbsPath) continue;

          const newRel = path.relative(path.dirname(fileUri.fsPath), newAbsPath);
          const newHref = (newRel.split(path.sep).join('/') || rawPath) + fragment;

          const hrefStart = lineText.indexOf(href, match.index);
          if (hrefStart < 0) continue;
          edit.replace(fileUri, new vscode.Range(lineIdx, hrefStart, lineIdx, hrefStart + href.length), newHref);
        }
      }
    }

    return edit;
  }

  private _registerRenameProvider(): vscode.Disposable {
    return vscode.languages.registerRenameProvider([{ language: 'markdown' }], {
      prepareRename: (document: vscode.TextDocument, position: vscode.Position): vscode.Range | undefined => {
        if (!this._isMarkdownFile(document.uri)) return undefined;
        const link = this._findMarkdownLinkAtPosition(document, position);
        if (link == null) return undefined;
        return link.hrefRange;
      },

      provideRenameEdits: async (
        document: vscode.TextDocument,
        position: vscode.Position,
        newName: string,
        _token: vscode.CancellationToken
      ): Promise<vscode.WorkspaceEdit | undefined> => {
        if (!this._isMarkdownFile(document.uri)) return undefined;
        const link = this._findMarkdownLinkAtPosition(document, position);
        if (link == null) return undefined;

        const oldAbs = resolveLinkTarget(link.href, document.uri.fsPath);
        if (oldAbs == null) return undefined;

        // newName is the user-supplied new relative href, interpreted from
        // the linking document's directory.
        const newAbs = path.isAbsolute(newName)
          ? path.normalize(newName)
          : path.normalize(path.resolve(path.dirname(document.uri.fsPath), newName));

        return this.buildFileMoveEdit(oldAbs, newAbs);
      }
    });
  }
}
