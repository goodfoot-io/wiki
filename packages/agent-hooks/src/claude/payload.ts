/**
 * Runtime boundary for the Claude Code PostToolUse payload.
 *
 * The SDK's `PostToolUseInput` cannot be trusted as a static type here: its
 * published declarations derive it from `@anthropic-ai/claude-agent-sdk`, which
 * `@goodfoot/agent-hooks` lists only as a devDependency, so the type collapses
 * to an `any`-valued index signature in consumers. The payload is untrusted
 * JSON at runtime regardless, so every field this hook reads is validated here
 * and nothing past this boundary sees the untyped input.
 */

/** The validated subset of a PostToolUse payload the wiki hook reads. */
export interface ClaudePostToolUsePayload {
  /** The session working directory relative tool paths resolve against. */
  cwd: string;
  /**
   * `tool_input.file_path` when the tool call carries one as a string; null
   * when the tool has no file target (e.g. NotebookEdit's `notebook_path`).
   */
  filePath: string | null;
}

/**
 * Validate the raw hook input. Returns null when the payload is not an object
 * or lacks a non-empty string `cwd` — a malformed payload the caller must
 * surface rather than act on. A missing or non-string `tool_input.file_path`
 * is not malformed: it yields `filePath: null`.
 */
export function parsePostToolUsePayload(input: unknown): ClaudePostToolUsePayload | null {
  if (typeof input !== 'object' || input === null || !('cwd' in input)) return null;
  const { cwd } = input;
  if (typeof cwd !== 'string' || cwd.length === 0) return null;
  return { cwd, filePath: readFilePath(input) };
}

/** Narrow `tool_input.file_path` to a non-empty string, or null. */
function readFilePath(input: object): string | null {
  if (!('tool_input' in input)) return null;
  const toolInput = input.tool_input;
  if (typeof toolInput !== 'object' || toolInput === null || !('file_path' in toolInput)) return null;
  const filePath = toolInput.file_path;
  return typeof filePath === 'string' && filePath.length > 0 ? filePath : null;
}
