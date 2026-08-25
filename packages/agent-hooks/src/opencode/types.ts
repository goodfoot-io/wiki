/**
 * Local structural types for the subset of opencode's plugin `Hooks` surface
 * this adapter uses. Deliberately loose (`unknown` leaves narrowed at the
 * edges): opencode's own SDK types are not a dependency — the bundle loads
 * in-process via file://, and every field is re-validated before use so a host
 * shape change fails open instead of misbehaving.
 *
 * Verified host facts these encode:
 * - `tool.execute.after(input{tool,sessionID,callID,args},
 *   output{title,output,metadata})` fires only after successful execution;
 *   appending to `output.output` injects context toward the model.
 */

/** The plugin init input this adapter reads. */
export interface OpencodePluginInput {
  /** Directory opencode resolved the plugin in; defaults to process cwd. */
  directory?: string;
}

/** Shared `input` envelope of the tool hooks. */
export interface OpencodeToolInput {
  /** Lowercase tool id (`edit`, `write`, `bash`, …). */
  tool?: string;
  sessionID?: string;
  callID?: string;
  /**
   * The tool-call arguments ride the *input* of the after hook. Narrowed
   * defensively at every use site.
   */
  args?: unknown;
}

/** After-hook mutable output: append-only injection channel + result metadata. */
export interface OpencodeAfterOutput {
  title?: unknown;
  /** The tool result text the model sees; appended to, never rewritten wholesale. */
  output?: string;
  metadata?: unknown;
}

/**
 * The hooks object this plugin returns to its host. Local and structural — no
 * dependency on an opencode SDK.
 */
export interface WikiOpencodeHooks {
  /** No-op-safe cleanup; must never throw. */
  dispose: () => void;
  'tool.execute.after': (input: OpencodeToolInput, output: OpencodeAfterOutput) => Promise<void>;
}
