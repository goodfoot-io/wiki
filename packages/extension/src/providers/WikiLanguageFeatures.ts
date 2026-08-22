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

import { readFile, stat } from 'node:fs/promises';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { type FrontmatterInfo, readFrontmatter } from '../utils/frontmatter.js';
import { getWikiLogger } from '../utils/logger.js';
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

/**
 * Match a standard markdown link `[label](href)` on a single line. The
 * regex skips images (`![...](...)`).
 */
const MARKDOWN_LINK_RE = /(?<!!)\[([^\]]*)\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g;

/** One scanned workspace markdown file: its URI and content split into lines. */
interface MarkdownFileScan {
  fileUri: vscode.Uri;
  lines: string[];
}

/** Max concurrent `readFile` calls during a single workspace scan pass. */
const SCAN_READ_CONCURRENCY = 6;

/**
 * Resolve a markdown link href to an absolute filesystem path, relative to
 * `fromFile`'s directory. Returns null for non-internal targets (http(s)/mailto/
 * fragment-only).
 *
 * A leading-`/` href is workspace-root-absolute (mirroring the Rust
 * resolver [resolve_link_path](../../../cli/src/commands/mod.rs)): the
 * leading slash is stripped and the remainder resolved against
 * `workspaceRoot`. This is detected explicitly, before `path.isAbsolute`,
 * because on POSIX `/foo` is filesystem-absolute and would otherwise be
 * mis-resolved to a non-existent root path.
 *
 * @param href          - Raw markdown link target.
 * @param fromFile      - Absolute path to the linking file.
 * @param workspaceRoot - Absolute path to the workspace root, used to
 *                        resolve workspace-root-absolute (`/`-rooted) links.
 * @returns The absolute target path, or null when the link is external/empty.
 */
export function resolveLinkTarget(href: string, fromFile: string, workspaceRoot?: string): string | null {
  if (href === '' || href.startsWith('#')) return null;
  if (/^[a-z][a-z0-9+.-]*:/i.test(href)) return null;
  const hashIdx = href.indexOf('#');
  const rawPath = hashIdx >= 0 ? href.slice(0, hashIdx) : href;
  if (rawPath === '') return null;
  if (rawPath.startsWith('/')) {
    const rest = rawPath.replace(/^\/+/, '');
    if (workspaceRoot != null) return path.normalize(path.resolve(workspaceRoot, rest));
    return path.normalize(rawPath);
  }
  if (path.isAbsolute(rawPath)) return path.normalize(rawPath);
  return path.normalize(path.resolve(path.dirname(fromFile), rawPath));
}

export class WikiLanguageFeatures {
  private readonly _checkDiagnostics: vscode.DiagnosticCollection;
  private readonly _disposables: vscode.Disposable[] = [];
  private readonly _frontmatterCache = new Map<string, { mtime: number; fm: FrontmatterInfo | null }>();

  /** Pending debounce timer handle — coalesces rapid saves into one check. */
  private _checkTimer: ReturnType<typeof setTimeout> | null = null;
  /** Whether a `wiki check` process is currently running. */
  private _checkRunning = false;
  /** Whether a save landed while a check was already running — triggers one more check. */
  private _checkPending = false;
  /** Whether dispose() has been called — guards against timer/async work after disposal. */
  private _disposed = false;
  /** Consecutive wiki check failures within a single check invocation. Capped at 3. */
  private _checkRetries = 0;

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
    if (this._checkTimer != null) {
      clearTimeout(this._checkTimer);
      this._checkTimer = null;
    }
    this._disposed = true;
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

  private async _runWikiJson<T>(args: string[], signal?: AbortSignal): Promise<T | null> {
    const wsRoot = this._workspaceRoot();
    try {
      const handle = await this._binaryManager.ready();
      const result = await runWikiCommand(handle.path, args, signal, wsRoot);
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
   * @param token - Cancellation token forwarded to `findFiles`; when
   *                cancelled, enumeration aborts and no files are returned.
   * @returns URIs of every workspace markdown file (excluding `node_modules`).
   */
  private async _allMarkdownFiles(token?: vscode.CancellationToken): Promise<vscode.Uri[]> {
    return vscode.workspace.findFiles('**/*.md', '**/node_modules/**', undefined, token);
  }

  // --------------------------------------------------------------------------
  // Completion
  // --------------------------------------------------------------------------

  /**
   * Read frontmatter for `absPath` through the mtime-keyed
   * {@link _frontmatterCache}. On cache miss the file is read from disk and
   * the result populated into the cache; on stat failure null is returned
   * without caching.
   *
   * @param absPath - Absolute filesystem path to a markdown file.
   * @returns Parsed frontmatter, or null when the file cannot be stat'd/read.
   */
  private async _cachedFrontmatter(absPath: string): Promise<FrontmatterInfo | null> {
    let mtime: number;
    try {
      const s = await stat(absPath);
      mtime = s.mtimeMs;
    } catch {
      return null;
    }

    const cached = this._frontmatterCache.get(absPath);
    if (cached !== undefined && cached.mtime === mtime) {
      return cached.fm;
    }

    const fm = await readFrontmatter(absPath);
    this._frontmatterCache.set(absPath, { mtime, fm });
    return fm;
  }

  /**
   * Drop every frontmatter-cache entry whose path was not seen in the given
   * enumeration of current workspace files (deleted files can never appear in
   * findFiles output). O(cache size) set difference.
   *
   * @param currentPaths - fsPaths of every markdown file currently enumerated.
   */
  private _evictStaleFrontmatter(currentPaths: readonly string[]): void {
    const seen = new Set(currentPaths);
    for (const key of this._frontmatterCache.keys()) {
      if (!seen.has(key)) {
        this._frontmatterCache.delete(key);
      }
    }
  }

  private _registerCompletionProvider(): vscode.Disposable {
    return vscode.languages.registerCompletionItemProvider(
      [{ language: 'markdown' }],
      {
        provideCompletionItems: async (
          document: vscode.TextDocument,
          position: vscode.Position,
          token: vscode.CancellationToken
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
          if (token.isCancellationRequested) return undefined;

          // Evict cache entries for files deleted since the last pass.
          this._evictStaleFrontmatter(files.map((fileUri) => fileUri.fsPath));

          type ResolvedEntry = {
            fileUri: vscode.Uri;
            href: string;
            fm: FrontmatterInfo | null;
          };

          const resolved = await Promise.all(
            files.map(async (fileUri): Promise<ResolvedEntry | null> => {
              const relPath = path.relative(sourceDir, fileUri.fsPath);
              if (relPath === '') return null;
              // Normalise separators to POSIX for markdown links.
              const href = relPath.split(path.sep).join('/');

              const fm = await this._cachedFrontmatter(fileUri.fsPath);
              return { fileUri, href, fm };
            })
          );

          if (token.isCancellationRequested) return undefined;

          const items: vscode.CompletionItem[] = [];
          for (const entry of resolved) {
            if (entry == null) continue;
            const ci = new vscode.CompletionItem(entry.href, vscode.CompletionItemKind.File);
            ci.insertText = entry.href;
            const fm = entry.fm;
            if (fm?.title != null && fm.summary != null) {
              ci.detail = fm.title;
              ci.documentation = new vscode.MarkdownString(fm.summary);
              // Sort wiki-aware files (with title + summary) before plain
              // markdown files.
              ci.sortText = `0_${fm.title.toLowerCase()}`;
            } else {
              ci.sortText = `1_${entry.href.toLowerCase()}`;
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

        const absTarget = resolveLinkTarget(link.href, document.uri.fsPath, this._workspaceRoot());
        if (absTarget == null) return undefined;

        // Serve frontmatter from the mtime-keyed cache exactly like the
        // completion path instead of re-reading the target on every hover.
        const fm = await this._cachedFrontmatter(absTarget);
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

  /**
   * Debounced and serialized diagnostics-on-save handler. Saves within 300 ms
   * of each other coalesce into a single `wiki check`; at most one check runs
   * at a time. When a save lands while a check is already running the handler
   * schedules exactly one follow-up check (latest-wins). All diagnostics in
   * the check output are published, clearing diagnostics for files that no
   * longer have errors.
   *
   * @returns Disposable that unregisters the save listener.
   */
  private _registerDiagnosticsOnSave(): vscode.Disposable {
    return vscode.workspace.onDidSaveTextDocument((document: vscode.TextDocument) => {
      if (!this._isMarkdownFile(document.uri)) return;

      if (this._checkRunning) {
        this._checkPending = true;
        return;
      }

      // Coalesce rapid saves (including "Save All") into one check by
      // resetting the debounce timer on every incoming save.
      if (this._checkTimer != null) {
        clearTimeout(this._checkTimer);
      }
      this._checkTimer = setTimeout(() => this._runCheck(), 300);
    });
  }

  /**
   * Run a single `wiki check` and publish diagnostics for every file in its
   * output. Loops while {@link _checkPending} is true so a save that lands
   * mid-check triggers exactly one follow-up run.
   *
   * @returns Promise that settles when the check-and-publish loop finishes.
   */
  private async _runCheck(): Promise<void> {
    this._checkTimer = null;
    this._checkRunning = true;
    this._checkRetries = 0;

    try {
      if (this._disposed) return;

      do {
        this._checkPending = false;

        if (this._disposed) return;

        // Each iteration gets a fresh AbortController + 30s timer so one
        // iteration's timeout does not permanently abort subsequent iterations.
        const abortController = new AbortController();
        const abortTimer = setTimeout(() => abortController.abort(), 30_000);

        try {
          // Racing _runWikiJson against an abort rejection ensures that
          // _binaryManager.ready() (called inside _runWikiJson) is also
          // bounded by the 30s timeout.
          const abortRejection = new Promise<never>((_, reject) => {
            if (abortController.signal.aborted) {
              reject(new DOMException('Aborted', 'AbortError'));
              return;
            }
            abortController.signal.addEventListener(
              'abort',
              () => {
                reject(new DOMException('Aborted', 'AbortError'));
              },
              { once: true }
            );
          });

          let output: CheckOutput | null = null;
          try {
            output = await Promise.race([
              this._runWikiJson<CheckOutput>(['check', '--format', 'json'], abortController.signal),
              abortRejection
            ]);
          } catch {
            output = null;
          }

          if (output == null) {
            this._checkRetries++;
            if (!this._disposed) {
              this._checkDiagnostics.clear();
            }
            console.warn('[wiki] wiki check returned no output (binary may be hung or unavailable).');
            if (this._checkRetries >= 3) {
              console.warn('[wiki] wiki check failed 3 consecutive times; giving up until next save.');
              break;
            }
            // Small delay before retry to avoid tight process-spawn loops.
            await new Promise((resolve) => setTimeout(resolve, 1000));
            continue;
          }

          // Reset retry counter on a successful check.
          this._checkRetries = 0;

          // Guard against unexpected JSON shape (e.g. {"error":"message"}).
          if (!Array.isArray(output.errors)) {
            console.warn('[wiki] wiki check returned unexpected JSON shape (missing errors array).');
            continue;
          }

          if (this._disposed) return;

          this._checkDiagnostics.clear();

          const byFile = new Map<string, vscode.Diagnostic[]>();
          for (const err of output.errors) {
            const diags = byFile.get(err.file) ?? [];
            const line = err.line > 0 ? err.line - 1 : 0;
            const range = new vscode.Range(line, 0, line, Number.MAX_SAFE_INTEGER);

            const diag = new vscode.Diagnostic(range, err.message, vscode.DiagnosticSeverity.Error);
            diag.source = 'wiki';
            diag.code = err.kind;
            diags.push(diag);
            byFile.set(err.file, diags);
          }

          for (const [file, diags] of byFile) {
            this._checkDiagnostics.set(vscode.Uri.file(file), diags);
          }
        } finally {
          clearTimeout(abortTimer);
        }
      } while (this._checkPending);
    } finally {
      this._checkRunning = false;
    }
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
        token: vscode.CancellationToken
      ): Promise<vscode.Location[] | undefined> => {
        if (!this._isMarkdownFile(document.uri)) return undefined;

        const link = this._findMarkdownLinkAtPosition(document, position);
        const targetPath =
          link != null ? resolveLinkTarget(link.href, document.uri.fsPath, this._workspaceRoot()) : document.uri.fsPath;
        if (targetPath == null) return undefined;

        return this._findIncomingLinks(targetPath, token);
      }
    });
  }

  /**
   * Enumerate every workspace markdown file once and read each file's content
   * once with bounded concurrency, producing the shared scan corpus used by
   * the incoming-link and file-move features.
   *
   * Files that cannot be read are skipped silently (prior sequential
   * behavior). Output order matches the {@link _allMarkdownFiles} enumeration
   * order regardless of read completion order.
   *
   * @param token - Cancellation token. When cancelled — during enumeration,
   *                which aborts before any file is read, or between
   *                individual reads — no further files are read.
   * @returns Scans for every readable workspace markdown file, or null when
   *          cancellation was observed; callers must treat null as "no
   *          results" rather than consuming a partial corpus.
   */
  private async _scanWorkspaceMarkdown(token?: vscode.CancellationToken): Promise<MarkdownFileScan[] | null> {
    const files = await this._allMarkdownFiles(token);

    // Bail before any read: cancellation landing during enumeration must not
    // lead to a partially-read corpus.
    if (token?.isCancellationRequested) return null;

    const slots: (MarkdownFileScan | null)[] = new Array(files.length).fill(null);
    let nextIndex = 0;

    const readNext = async (): Promise<void> => {
      while (nextIndex < files.length) {
        if (token?.isCancellationRequested) return;
        const index = nextIndex;
        nextIndex++;
        const fileUri = files[index]!;
        try {
          const text = await readFile(fileUri.fsPath, 'utf8');
          slots[index] = { fileUri, lines: text.split('\n') };
        } catch {
          // Unreadable file — skip, matching prior behavior.
        }
      }
    };

    const workerCount = Math.min(SCAN_READ_CONCURRENCY, files.length);
    const workers: Promise<void>[] = [];
    for (let i = 0; i < workerCount; i++) {
      workers.push(readNext());
    }
    await Promise.all(workers);

    if (token?.isCancellationRequested) return null;

    return slots.filter((scan): scan is MarkdownFileScan => scan !== null);
  }

  /**
   * Scan every markdown file in the workspace for a link whose resolved
   * absolute path equals `targetAbsPath`.
   *
   * Performs ONE enumeration+read pass over the workspace corpus.
   *
   * @param targetAbsPath - Absolute path to the file being referenced.
   * @param token - Cancellation token; when cancellation is observed, no
   *                locations are returned.
   * @returns Locations of every matching link, or an empty array when the
   *          scan was cancelled — never a partial set.
   */
  private async _findIncomingLinks(
    targetAbsPath: string,
    token?: vscode.CancellationToken
  ): Promise<vscode.Location[]> {
    const scans = await this._scanWorkspaceMarkdown(token);

    // Fail closed: a cancelled reference request must never present a subset
    // of the references as the complete answer, mirroring
    // {@link WikiLanguageFeatures.buildFileMoveEdit}.
    if (scans === null) return [];

    const locations: vscode.Location[] = [];
    const wsRoot = this._workspaceRoot();
    // One scanner instance for the whole pass; reset between lines.
    const re = new RegExp(MARKDOWN_LINK_RE.source, 'g');

    for (const { fileUri, lines } of scans) {
      for (let lineIdx = 0; lineIdx < lines.length; lineIdx++) {
        const lineText = lines[lineIdx]!;
        re.lastIndex = 0;
        let match: RegExpExecArray | null;
        for (match = re.exec(lineText); match !== null; match = re.exec(lineText)) {
          const href = match[2]!;
          const resolved = resolveLinkTarget(href, fileUri.fsPath, wsRoot);
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
   * @param token - Cancellation token; when cancellation is observed mid-scan
   *                the returned promise rejects with `vscode.CancellationError`
   *                rather than yielding a partially-built edit.
   * @returns A WorkspaceEdit replacing every matching href.
   */
  async buildFileMoveEdit(
    oldAbsPath: string,
    newAbsPath: string,
    token?: vscode.CancellationToken
  ): Promise<vscode.WorkspaceEdit> {
    const edit = new vscode.WorkspaceEdit();
    const scans = await this._scanWorkspaceMarkdown(token);

    // Fail closed: a cancelled rename must never apply a subset of the
    // rewrites, so cancellation surfaces as a rejected promise.
    if (scans === null) throw new vscode.CancellationError();

    this._appendMoveRewriteEdits(scans, new Map([[oldAbsPath, newAbsPath]]), edit);
    return edit;
  }

  /**
   * Pure computation over an already-scanned corpus: rewrite every markdown
   * link whose resolved absolute target matches one of `moves`' keys so that
   * its href becomes a relative path from the linking file's directory to
   * the mapped new absolute path. Fragments (`#anchors`) are preserved and
   * re-appended to the rewritten href.
   *
   * A single traversal handles every move pair at once, so a directory
   * rename with F files costs one workspace pass instead of F passes.
   *
   * @param scans - Shared scan corpus (one entry per readable workspace .md).
   * @param moves - Map of old absolute link-target path → new absolute path.
   * @param edit - WorkspaceEdit accumulating the replacement edits.
   * @returns The number of replacement edits appended.
   */
  private _appendMoveRewriteEdits(
    scans: readonly MarkdownFileScan[],
    moves: ReadonlyMap<string, string>,
    edit: vscode.WorkspaceEdit
  ): number {
    let rewriteCount = 0;
    if (moves.size === 0) return rewriteCount;
    const wsRoot = this._workspaceRoot();
    // One scanner instance for the whole pass; reset between lines.
    const re = new RegExp(MARKDOWN_LINK_RE.source, 'g');

    for (const { fileUri, lines } of scans) {
      const sourceDir = path.dirname(fileUri.fsPath);
      for (let lineIdx = 0; lineIdx < lines.length; lineIdx++) {
        const lineText = lines[lineIdx]!;
        re.lastIndex = 0;
        let match: RegExpExecArray | null;
        for (match = re.exec(lineText); match !== null; match = re.exec(lineText)) {
          const href = match[2]!;
          const hashIdx = href.indexOf('#');
          const rawPath = hashIdx >= 0 ? href.slice(0, hashIdx) : href;
          const fragment = hashIdx >= 0 ? href.slice(hashIdx) : '';
          const resolved = resolveLinkTarget(rawPath, fileUri.fsPath, wsRoot);
          if (resolved === null) continue;
          const newAbsPath = moves.get(resolved);
          if (newAbsPath === undefined) continue;

          const newRel = path.relative(sourceDir, newAbsPath);
          const newHref = (newRel.split(path.sep).join('/') || rawPath) + fragment;

          const hrefStart = lineText.indexOf(href, match.index);
          if (hrefStart < 0) continue;
          edit.replace(fileUri, new vscode.Range(lineIdx, hrefStart, lineIdx, hrefStart + href.length), newHref);
          rewriteCount++;
        }
      }
    }
    return rewriteCount;
  }

  /**
   * Build a WorkspaceEdit that rewrites incoming links to every `.md` file
   * inside a renamed directory, in a single pass over the workspace.
   *
   * Enumerates `.md` files under `newDirPath` to derive one old→new absolute
   * path pair per moved page, then performs ONE enumeration+read+scan pass
   * over the workspace corpus ({@link _scanWorkspaceMarkdown}) computing every
   * edit in a single traversal via {@link _appendMoveRewriteEdits}. Cost is
   * proportional to the workspace regardless of how many pages moved — a
   * directory move of K pages performs one enumeration and one read pass
   * (~N reads), not K full-workspace rescans (K×N reads).
   *
   * When a directory is renamed in the VS Code explorer the
   * `onDidRenameFiles` event delivers a single event for the directory URI;
   * this method bridges that event to link rewriting.
   *
   * @param oldDirPath - Absolute path of the directory before the rename.
   * @param newDirPath - Absolute path of the directory after the rename.
   * @returns A WorkspaceEdit that rewrites every link targeting a file inside the renamed directory.
   */
  async buildDirectoryMoveEdit(oldDirPath: string, newDirPath: string): Promise<vscode.WorkspaceEdit> {
    const mdFiles = await vscode.workspace.findFiles(
      new vscode.RelativePattern(vscode.Uri.file(newDirPath), '**/*.md'),
      '**/node_modules/**'
    );
    if (mdFiles.length === 0) return new vscode.WorkspaceEdit();

    // Map each moved page's pre-rename absolute path to its post-rename path.
    // Keys are built with the same join the per-page implementation passed as
    // oldAbsPath, so lookups below compare against exactly the strings
    // resolveLinkTarget() output was compared against before.
    const moves = new Map<string, string>();
    for (const fileUri of mdFiles) {
      const relPath = path.relative(newDirPath, fileUri.fsPath);
      moves.set(path.join(oldDirPath, relPath), fileUri.fsPath);
    }

    const edit = new vscode.WorkspaceEdit();
    // No token today (onDidRenameFiles has none to give), so null cannot
    // occur — but fail closed anyway: a cancelled corpus must never yield
    // partial directory-move rewrites.
    const scans = await this._scanWorkspaceMarkdown();
    if (scans === null) return edit;
    const rewriteCount = this._appendMoveRewriteEdits(scans, moves, edit);
    getWikiLogger().debug(
      'directory move %s -> %s: %d pages moved, scanned %d markdown files, rewrote %d links',
      oldDirPath,
      newDirPath,
      moves.size,
      scans.length,
      rewriteCount
    );
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
        token: vscode.CancellationToken
      ): Promise<vscode.WorkspaceEdit | undefined> => {
        if (!this._isMarkdownFile(document.uri)) return undefined;
        const link = this._findMarkdownLinkAtPosition(document, position);
        if (link == null) return undefined;

        const oldAbs = resolveLinkTarget(link.href, document.uri.fsPath, this._workspaceRoot());
        if (oldAbs == null) return undefined;

        // newName is the user-supplied new relative href, interpreted from
        // the linking document's directory.
        const newAbs = path.isAbsolute(newName)
          ? path.normalize(newName)
          : path.normalize(path.resolve(path.dirname(document.uri.fsPath), newName));

        return this.buildFileMoveEdit(oldAbs, newAbs, token);
      }
    });
  }
}
