import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { postToolUseHook, postToolUseOutput } from '@goodfoot/claude-code-hooks';

interface CheckDiagnostic {
  kind: string;
  file: string;
  line: number;
  message: string;
}

interface ScaffoldAnchor {
  path: string;
  startLine: number;
  endLine: number;
}

interface ScaffoldMesh {
  slug: string;
  headingChain: string[];
  sectionOpening: string[];
  anchors: ScaffoldAnchor[];
}

interface ScaffoldPage {
  path: string;
  title: string;
  meshes: ScaffoldMesh[];
}

interface ScaffoldParseError {
  path: string;
  category: string;
  message: string;
}

interface ScaffoldOutput {
  schemaVersion: number;
  parseErrors: ScaffoldParseError[];
  pages: ScaffoldPage[];
}

interface CommandResult {
  ok: boolean;
  status: number;
  stdout: string;
  stderr: string;
}

function run(cmd: string, args: string[]): CommandResult {
  const r = spawnSync(cmd, args, { encoding: 'utf8' });
  return {
    ok: r.status === 0,
    status: r.status ?? -1,
    stdout: r.stdout ?? '',
    stderr: r.stderr ?? ''
  };
}

function readJsonArray<T>(path: string): T[] {
  if (!existsSync(path)) return [];
  try {
    const v = JSON.parse(readFileSync(path, 'utf8'));
    return Array.isArray(v) ? (v as T[]) : [];
  } catch {
    return [];
  }
}

function diagKey(d: CheckDiagnostic): string {
  return JSON.stringify([d.kind, d.file, d.line, d.message]);
}

function meshKey(m: ScaffoldMesh): string {
  const anchors = m.anchors
    .map((a) => [a.path, a.startLine, a.endLine] as const)
    .slice()
    .sort((a, b) => {
      if (a[0] !== b[0]) return a[0] < b[0] ? -1 : 1;
      if (a[1] !== b[1]) return a[1] - b[1];
      return a[2] - b[2];
    });
  return JSON.stringify([m.slug, anchors]);
}

function renderDiagnostics(diags: CheckDiagnostic[]): string {
  return diags.map((d) => `**${d.kind}** — \`${d.file}:${d.line}\`\n${d.message}\n`).join('\n');
}

function renderMesh(mesh: ScaffoldMesh): string {
  const lines: string[] = [];
  if (mesh.headingChain.length) {
    lines.push(`### ${mesh.headingChain.join(' → ')}`);
  }
  for (const line of mesh.sectionOpening) {
    lines.push(`> ${line}`);
  }
  if (lines.length) lines.push('');
  lines.push('```bash');
  lines.push(`git mesh add ${mesh.slug} \\`);
  const anchors = mesh.anchors.map((a) => `${a.path}#L${a.startLine}-L${a.endLine}`);
  for (let i = 0; i < anchors.length; i++) {
    const cont = i < anchors.length - 1 ? ' \\' : '';
    lines.push(`  ${anchors[i]}${cont}`);
  }
  lines.push(`git mesh why ${mesh.slug} -m "[why]"`);
  lines.push('```');
  return lines.join('\n');
}

function renderScaffolds(
  byPage: Map<string, { page: ScaffoldPage; meshes: ScaffoldMesh[] }>,
  parseErrors: ScaffoldParseError[]
): string {
  const pageBlocks: string[] = [];
  for (const { page, meshes } of byPage.values()) {
    const header = page.title ? `${page.title} • ${page.path}` : page.path;
    const meshBlocks = meshes.map(renderMesh).join('\n\n');
    pageBlocks.push(`## ${header}\n\n${meshBlocks}`);
  }
  let body = pageBlocks.join('\n\n---\n\n');
  if (parseErrors.length) {
    const sep = body ? '\n\n' : '';
    body +=
      `${sep}Unable to generate scaffolding due to parsing errors:\n` +
      parseErrors.map((e) => `- ${e.path}: ${e.message}`).join('\n');
  }
  return body;
}

export default postToolUseHook(
  { matcher: 'Edit|Write|MultiEdit|NotebookEdit', timeout: 30000 },
  (input, { logger }) => {
    const sessionId = input.session_id;
    if (!sessionId) return postToolUseOutput({});

    const touched = run('git', ['mesh', 'advice', sessionId, 'touched']);
    if (!touched.ok) {
      logger.info('git mesh advice unavailable; skipping', {
        status: touched.status
      });
      return postToolUseOutput({});
    }
    const files = touched.stdout
      .split('\n')
      .map((s) => s.trim())
      .filter(Boolean);
    if (!files.length) return postToolUseOutput({});

    const checkRes = run('wiki', ['check', '--format', 'json', '--no-exit-code', ...files]);
    let allDiags: CheckDiagnostic[] = [];
    if (checkRes.stdout) {
      try {
        const j = JSON.parse(checkRes.stdout) as { errors?: CheckDiagnostic[] };
        allDiags = Array.isArray(j.errors) ? j.errors : [];
      } catch {
        logger.warn('wiki check produced non-JSON output', {
          stderr: checkRes.stderr
        });
      }
    }

    const stateDir = `/tmp/wiki/session/${sessionId}`;
    mkdirSync(stateDir, { recursive: true });
    const diagFile = join(stateDir, 'diagnostics');
    const scafFile = join(stateDir, 'scaffolds');

    const priorDiags = readJsonArray<CheckDiagnostic>(diagFile);
    const priorDiagKeys = new Set(priorDiags.map(diagKey));
    const newDiags = allDiags.filter((d) => !priorDiagKeys.has(diagKey(d)));

    const wikiPages = [...new Set(newDiags.map((d) => d.file).filter((f) => f.endsWith('.md')))];

    let scafPages: ScaffoldPage[] = [];
    let parseErrors: ScaffoldParseError[] = [];
    if (wikiPages.length) {
      const scafRes = run('wiki', ['scaffold', '--format', 'json', ...wikiPages]);
      if (scafRes.stdout) {
        try {
          const j = JSON.parse(scafRes.stdout) as ScaffoldOutput;
          scafPages = Array.isArray(j.pages) ? j.pages : [];
          parseErrors = Array.isArray(j.parseErrors) ? j.parseErrors : [];
        } catch {
          logger.warn('wiki scaffold produced non-JSON output', {
            stderr: scafRes.stderr
          });
        }
      }
    }

    const allMeshes: { page: ScaffoldPage; mesh: ScaffoldMesh }[] = [];
    for (const page of scafPages) {
      for (const mesh of page.meshes) {
        allMeshes.push({ page, mesh });
      }
    }

    const priorScafs = readJsonArray<ScaffoldMesh>(scafFile);
    const priorScafKeys = new Set(priorScafs.map(meshKey));
    const newScafs = allMeshes.filter((s) => !priorScafKeys.has(meshKey(s.mesh)));

    const haveScaffoldOutput = newScafs.length > 0 || parseErrors.length > 0;
    const renderedDiags = haveScaffoldOutput ? newDiags.filter((d) => d.kind !== 'mesh_uncovered') : newDiags;

    const sections: string[] = [];
    if (renderedDiags.length) {
      sections.push(`# Wiki Validation Errors\n\n${renderDiagnostics(renderedDiags)}`);
    }
    if (haveScaffoldOutput) {
      const byPage = new Map<string, { page: ScaffoldPage; meshes: ScaffoldMesh[] }>();
      for (const { page, mesh } of newScafs) {
        let entry = byPage.get(page.path);
        if (!entry) {
          entry = { page, meshes: [] };
          byPage.set(page.path, entry);
        }
        entry.meshes.push(mesh);
      }
      sections.push(`# Git Mesh Coverage Suggestions\n\n${renderScaffolds(byPage, parseErrors)}`);
    }

    if (newDiags.length) {
      writeFileSync(diagFile, JSON.stringify([...priorDiags, ...newDiags]));
    } else if (!existsSync(diagFile)) {
      writeFileSync(diagFile, '[]');
    }
    if (newScafs.length) {
      writeFileSync(scafFile, JSON.stringify([...priorScafs, ...newScafs.map((s) => s.mesh)]));
    } else if (!existsSync(scafFile)) {
      writeFileSync(scafFile, '[]');
    }

    if (!sections.length) return postToolUseOutput({});

    const body = sections.join('\n\n');
    return postToolUseOutput({
      systemMessage: body,
      hookSpecificOutput: { additionalContext: body }
    });
  }
);
