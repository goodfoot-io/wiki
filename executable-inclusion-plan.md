# Executable Inclusion Plan

## Goal

Ship the `wiki` CLI with the VS Code extension in `packages/extension` without requiring a separate user-managed CLI install, while making the binary available on the VS Code integrated terminal `PATH`.

The recommended approach is:

1. Keep the extension as a single cross-platform VSIX.
2. Reuse the existing CLI release assets built from `packages/cli` and published by the CLI release workflow.
3. Download and install the correct native binary on first activation of the extension.
4. Store the managed binary in extension-owned storage.
5. Prepend the managed binary directory to the integrated terminal `PATH` using the VS Code terminal environment API.
6. Resolve and spawn the managed binary by absolute path inside the extension instead of relying on ambient `PATH`.

This plan avoids shipping every platform binary inside one VSIX, avoids per-platform extension packaging, and fits the actual extension lifecycle constraints: VS Code extensions do not get a user-install-time npm `postinstall` hook.

## Why This Plan Fits The Current Repo

The current repository already has the hard parts of binary production and versioning:

- `packages/cli/package.json` defines `@goodfoot/wiki` as a meta-package with platform-specific optional dependencies.
- `packages/cli/scripts/postinstall.js` already maps `process.platform` and `process.arch` to platform packages and installs the native binary into `packages/cli/lib`.
- `.github/workflows/release-cli.yml` already builds release binaries for Linux x64/arm64, macOS x64/arm64, and Windows x64, publishes npm platform packages, and uploads GitHub release assets.
- `packages/extension` currently assumes `wiki` is already on `PATH`, so the missing piece is extension-side binary management rather than binary production.

This means the extension does not need a new binary build system. It needs a controlled bootstrap path that consumes the artifacts the CLI release process already emits.

## Constraints And Design Principles

### Extension Lifecycle Constraint

The extension cannot depend on npm `postinstall` behavior at end-user install time. `vscode:prepublish` runs only when packaging or publishing the extension, not when a user installs the VSIX or Marketplace build.

Implication:

- "Install on extension install" must be implemented as "install on first activation" or "install on startup activation if the managed binary is missing."

### Fail-Closed Behavior

The repo conventions prefer fail-closed behavior. The extension should not silently fall back to ambiguous or partially configured states.

Implication:

- If the platform is unsupported, say so explicitly.
- If the download fails, surface a clear actionable error.
- If checksum verification fails, reject the binary.
- If the managed binary is required for extension features, block those features until installation succeeds.

### Single Source Of Truth For Versioning

The repo already version-locks the CLI and extension releases.

Implication:

- The extension should fetch the CLI binary that matches the extension version by default.
- The extension should not independently select "latest" CLI versions at runtime.

### Managed Over Ambient Execution

The extension should prefer its own managed binary and use ambient `PATH` only as an explicit development fallback.

Implication:

- Runtime behavior remains deterministic.
- The extension can guarantee that the binary version matches the extension release.
- Terminal integration becomes a convenience layer instead of the extension's own dependency resolution mechanism.

## Recommended Architecture

## 1. Binary Source Of Truth

Use the CLI GitHub release assets produced by `.github/workflows/release-cli.yml` as the extension download source.

Expected asset mapping:

- `wiki-linux-x64`
- `wiki-linux-arm64`
- `wiki-darwin-x64`
- `wiki-darwin-arm64`
- `wiki-win32-x64.exe`

The extension should derive the expected asset name from:

- `process.platform`
- `process.arch`
- the extension version in `packages/extension/package.json`

The resolved tag should be:

- `wiki-v${extensionVersion}`

Rationale:

- This reuses existing release outputs.
- It preserves the current CLI/extension version lock.
- It avoids inventing a second binary-distribution channel.

## 2. Managed Install Location

Store the downloaded binary under extension-owned persistent storage, not in the extension installation directory.

Recommended layout:

```text
<globalStorage>/bin/<version>/<platform>-<arch>/wiki
<globalStorage>/bin/<version>/<platform>-<arch>/wiki.exe
<globalStorage>/manifests/<version>/<platform>-<arch>.json
```

Use `ExtensionContext.globalStorageUri` as the root.

Rationale:

- The extension install directory may be replaced on update.
- Global storage is intended for extension-managed persistent state.
- Versioned directories allow atomic upgrades and easy rollback logic.

## 3. Binary Resolver Layer

Replace the current `wikiBinary.ts` implementation with a resolver that understands managed binaries.

Recommended resolution order:

1. Managed binary for current extension version.
2. Development fallback from `PATH`.
3. No binary available.

The resolver should return structured results, not just a string:

```ts
type WikiBinaryResolution =
  | { kind: 'managed'; path: string; version: string }
  | { kind: 'path'; path: string }
  | { kind: 'missing'; reason: string };
```

Do not continue spawning `'wiki'` directly. Every extension call site should receive an absolute executable path from the resolver or installer.

Rationale:

- The current implementation in `packages/extension/src/utils/wikiBinary.ts` uses `which wiki` and `spawn('wiki', ...)`, which cannot support managed installation.
- A structured resolver makes diagnostics, UI, and tests much cleaner.

## 4. Installer Service

Add a dedicated installer module in the extension host, for example:

- `packages/extension/src/utils/wikiInstaller.ts`

Responsibilities:

1. Detect supported platform/arch pairs.
2. Build the expected release URL for the exact extension version.
3. Download the asset to a temporary file.
4. Verify checksum before activation of the binary.
5. Move the verified file into the versioned managed install location atomically.
6. Set executable permissions on Unix.
7. Persist a small install manifest with source URL, checksum, platform, arch, and installed version.

The installer should be idempotent:

- If the correct verified binary is already present, do nothing.
- If an incomplete or corrupt install exists, remove and reinstall.

Recommended flow:

1. Compute target install location.
2. Check for a valid manifest and executable.
3. If not valid, download to `<target>.download`.
4. Verify checksum from a trusted manifest.
5. Rename into final place.
6. Write manifest only after the binary is finalized.

Rationale:

- This handles interrupted installs safely.
- It avoids treating a partially written file as installed.

## 5. Checksum And Integrity Model

Do not trust the download solely because it came from GitHub. Add explicit integrity verification.

Recommended release additions:

1. Generate a checksum manifest in the CLI release workflow, for example:

```json
{
  "version": "0.5.16",
  "assets": {
    "linux-x64": {
      "name": "wiki-linux-x64",
      "sha256": "..."
    },
    "linux-arm64": {
      "name": "wiki-linux-arm64",
      "sha256": "..."
    },
    "darwin-x64": {
      "name": "wiki-darwin-x64",
      "sha256": "..."
    },
    "darwin-arm64": {
      "name": "wiki-darwin-arm64",
      "sha256": "..."
    },
    "win32-x64": {
      "name": "wiki-win32-x64.exe",
      "sha256": "..."
    }
  }
}
```

2. Publish that manifest as a GitHub release asset.
3. Have the extension download the checksum manifest first, then the platform asset.

If stronger provenance is needed later, add signature verification, but checksum verification is the minimum bar for a fail-closed managed binary install.

## 6. Activation Strategy

Keep the current activation event `onStartupFinished`, but use activation to bootstrap the managed binary asynchronously.

Recommended activation flow in `packages/extension/src/extension.ts`:

1. Start extension activation.
2. Create installer/resolver services.
3. Kick off `ensureManagedWikiBinary(context)` in the background.
4. Register the custom editor and commands immediately.
5. Make command handlers and provider code await binary readiness when they need the CLI.

Two valid implementation styles:

### Option A: Background Install With On-Demand Await

- Activation starts the install task.
- Consumers await `binaryManager.ready()` before running CLI commands.

Pros:

- Faster startup.
- No activation stall for users who never use CLI-backed features immediately.

Cons:

- First interactive use may still block on install.

### Option B: Activation Blocks Until Install Completes

- Activation waits for installer completion before registering CLI-backed features.

Pros:

- Simpler runtime model.
- No deferred readiness handling.

Cons:

- Slower extension startup.
- Higher activation failure blast radius.

Recommendation:

- Use Option A.

This keeps startup lightweight while preserving deterministic behavior for features that actually need the CLI.

## 7. Terminal PATH Injection

Use `context.environmentVariableCollection` to prepend the managed binary directory to `PATH`.

Recommended behavior:

1. After successful binary installation, compute the directory containing the executable.
2. Prepend that directory to `PATH` using the environment variable collection.
3. Persist the collection so future terminals inherit the path without reinstalling the binary.
4. Update the collection on extension upgrade if the managed path changes to a versioned directory.

Important limits:

- This affects VS Code integrated terminals, not the user’s global shell configuration.
- Existing terminal sessions may need to be recreated to pick up the new path.
- This should be described clearly in user-facing messaging.

Recommended UX:

- After a successful first install, show a notification:
  - "`wiki` is installed for this extension. New integrated terminals will have it on PATH."
- Do not edit shell rc files such as `.bashrc`, `.zshrc`, or PowerShell profiles.

Rationale:

- VS Code already provides the right scope-specific mechanism.
- Editing user shell config would be intrusive and error-prone.

## 8. Runtime Command Execution

Change all extension CLI calls to use the resolved executable path directly.

Current state:

- `packages/extension/src/utils/wikiBinary.ts` spawns `'wiki'`.

Target state:

- `runWikiCommand(binaryPath, args, signal, cwd)` spawns the absolute managed path.

Suggested API shape:

```ts
interface WikiBinaryHandle {
  path: string;
  source: 'managed' | 'path';
}

async function getWikiBinary(): Promise<WikiBinaryHandle>;
async function runWikiCommand(handle: WikiBinaryHandle, args: string[], signal?: AbortSignal, cwd?: string);
```

This change should propagate to:

- `packages/extension/src/commands/wikiQuickPick.ts`
- `packages/extension/src/providers/WikiEditorProvider.ts`
- any future command or provider that shells out to the CLI

Rationale:

- The terminal `PATH` integration is useful for users, but the extension itself should not depend on that side effect.

## 9. Remote Development Behavior

Decide this explicitly before implementation. There are two materially different scopes.

### Scope 1: Local Extension Host Only

Behavior:

- Managed binary installs only in the local extension host environment.
- Integrated terminals launched in a local window get the PATH entry.
- Remote SSH / WSL / dev container scenarios are not supported initially.

Pros:

- Simpler implementation.
- Faster time to ship.

Cons:

- Users in remote workspaces will see inconsistent behavior.

### Scope 2: Support Remote Extension Hosts

Behavior:

- The extension installs the binary in whichever extension host is running the extension.
- Terminal PATH injection applies in remote windows too.

Requirements:

- Audit `extensionKind`.
- Test on SSH, WSL, and dev containers.
- Confirm that CLI execution should happen close to the workspace, not always on the local machine.

Recommendation:

- Treat remote support as a deliberate phase-two target unless it is a release requirement now.

Reason:

- The extension currently has no explicit remote-host packaging or test story.
- Binary bootstrap in remote contexts is a broader behavioral decision than a packaging tweak.

## 10. Unsupported Platforms

Match the current CLI binary matrix exactly. If a platform is not in the release matrix, the extension should fail closed with a specific message.

Current supported targets:

- `linux-x64`
- `linux-arm64`
- `darwin-x64`
- `darwin-arm64`
- `win32-x64`

Recommended unsupported-platform behavior:

- Show a single actionable error:
  - "`wiki` is not available for <platform>-<arch> in this release."
- Do not attempt source builds from inside the extension.
- Do not silently fall back to partial functionality.

## Release Pipeline Changes

## 1. CLI Release Workflow

Extend `.github/workflows/release-cli.yml` so the GitHub release becomes consumable by the extension installer.

Additions:

1. Generate a checksums manifest for all built assets.
2. Upload the checksum manifest as a release asset.
3. Keep the existing binary asset naming stable.

Strong recommendation:

- Do not change asset names casually after the extension ships.
- If asset names must change, version the manifest schema explicitly and update the extension installer accordingly.

## 2. Extension Release Workflow

The extension release workflow can remain a single-platform packaging job because the extension artifact remains cross-platform.

Additions:

1. Add a validation step that confirms the corresponding CLI release assets exist for the extension version before publishing the extension.
2. Fail the extension publish job if the matching CLI tag or release assets are missing.

Rationale:

- This prevents publishing an extension version that points to nonexistent CLI assets.

Recommended order of operations:

1. Publish CLI release for version `X`.
2. Confirm GitHub release assets and checksum manifest exist.
3. Publish extension version `X`.

## 3. Version Contract

Document and enforce the following contract:

- Extension version `X.Y.Z` downloads release `wiki-vX.Y.Z`.

Do not make this dynamic by default. A runtime "latest" lookup would introduce nondeterminism and break reproducibility.

## Code Changes By Area

## 1. `packages/extension/src/utils/wikiBinary.ts`

Replace the current path-only logic with:

- platform/arch detection
- managed install path resolution
- PATH fallback only for development
- absolute-path spawning

This file may become too broad. Splitting it is likely cleaner:

- `wikiPlatform.ts`
- `wikiBinary.ts`
- `wikiInstaller.ts`

## 2. `packages/extension/src/extension.ts`

Add startup orchestration:

- create binary manager
- start background install
- register commands/providers with access to the binary manager
- initialize terminal PATH injection after install success

Avoid global mutable module state if possible. Prefer passing a service object into constructors and command closures.

## 3. `packages/extension/src/providers/WikiEditorProvider.ts`

Update the provider to:

- await the managed binary handle before first render
- show installer progress or a clear loading/error state when install is pending or failed
- stop checking only for `findWikiBinary()` on ambient `PATH`

The current error message:

- "wiki binary not found on PATH. Install the wiki CLI and reload the window."

should become something like:

- "Installing wiki CLI…"
- then either continue rendering or surface:
  - "Failed to install wiki CLI for this extension: <reason>"

## 4. `packages/extension/src/commands/wikiQuickPick.ts`

Update command execution to:

- await binary readiness
- use the resolved absolute executable path
- distinguish installer failures from CLI command failures

This matters for UX. A failed download is not the same class of error as a failed `wiki search`.

## 5. Test Infrastructure

`packages/extension/test/runTest.ts` currently fails if `wiki` is not on `PATH`.

That assumption must be removed.

Recommended test changes:

1. Add a fake or fixture binary for unit-style tests of resolution and spawning behavior.
2. Add installer tests that mock release downloads and checksum manifests.
3. Update integration tests so they provision the managed binary into the test extension storage area.
4. Reserve ambient `PATH` fallback only for explicit local-development scenarios.

If real network downloads are part of tests, the test suite becomes slower and more brittle. Prefer mockable installer logic with deterministic local fixtures.

## 6. User-Facing Configuration

Add minimal configuration only if it is needed for development or enterprise overrides.

Possible settings:

- `wiki.binary.source`
  - default: `managed`
- `wiki.binary.releaseBaseUrl`
  - default: GitHub release base URL
- `wiki.binary.usePathFallback`
  - default: `false` in production behavior, enabled only for development if needed

Recommendation:

- Keep the public settings surface as small as possible for the first release.
- If overrides are added, they should be advanced settings, not part of the normal user flow.

## UX Plan

## First Install

Expected user experience:

1. Extension activates.
2. Extension ensures the managed binary exists.
3. On success:
   - CLI-backed features work.
   - new integrated terminals have `wiki` on `PATH`.
4. On failure:
   - extension features that depend on the CLI show a direct error with a retry path.

Recommended notifications:

- On first successful install:
  - concise information only
- On failure:
  - explicit failure reason
  - a retry command
  - link to logs if available

## Upgrade

Expected behavior:

1. Extension `X+1` activates.
2. It installs binary version `X+1` into a new versioned directory.
3. It updates terminal PATH injection to point at the new version directory.
4. It can garbage-collect old managed versions after successful install, either immediately or on a later cleanup pass.

Recommended cleanup policy:

- Keep the current version and at most one previous version until cleanup is proven safe.

## Offline Behavior

This approach requires network access the first time a version is installed.

Recommended explicit policy:

- If the managed binary is already installed, offline usage works.
- If it is not installed and the network is unavailable, fail with a clear error.

Do not try to hide this limitation. If offline first-install support becomes a hard requirement later, that is the point where platform-specific VSIX bundling becomes worth reconsidering.

## Risks And Mitigations

## Risk 1: Extension Publishes Before CLI Assets Are Available

Impact:

- Fresh installs fail to download binaries.

Mitigation:

- Add extension-release validation against CLI release assets.

## Risk 2: Asset Name Drift Breaks Installer Logic

Impact:

- Installer cannot locate binary assets.

Mitigation:

- Treat release asset names as an explicit API contract.
- Verify expected asset names in CI.

## Risk 3: Partial Or Corrupt Install Leaves Broken State

Impact:

- Extension thinks a binary exists but cannot execute it.

Mitigation:

- Use temp files, checksum verification, atomic rename, and manifest-after-success semantics.

## Risk 4: Terminal PATH Update Does Not Affect Existing Shells

Impact:

- Users think install failed because old terminals still lack `wiki`.

Mitigation:

- Message clearly that new integrated terminals receive the updated PATH.

## Risk 5: Remote Host Behavior Is Surprising

Impact:

- Local and remote windows behave differently.

Mitigation:

- Either scope the first release to local extension hosts or explicitly test and document remote support.

## Risk 6: Managed Binary Logic Becomes Hard To Test

Impact:

- Shipping regressions in binary installation or resolution.

Mitigation:

- Isolate platform detection, URL construction, checksum verification, installer IO, and process spawning into separately testable units.

## Phased Rollout

## Phase 1: Core Managed Binary Support

Deliverables:

- binary resolver
- binary installer
- checksum verification
- absolute-path spawning
- integrated terminal PATH injection
- updated error handling

Exit criteria:

- extension works on all currently supported platforms without a preinstalled `wiki`
- new integrated terminals can run `wiki`

## Phase 2: Release Hardening

Deliverables:

- CLI checksum manifest generation
- extension-release validation against CLI assets
- cleanup of old managed versions
- improved diagnostics and retry flow

Exit criteria:

- no version can be published with missing binary artifacts

## Phase 3: Remote Support

Deliverables:

- explicit remote-host behavior
- `extensionKind` decision
- SSH / WSL / dev container verification

Exit criteria:

- supported remote modes are documented and tested

## Non-Goals For The First Implementation

- Bundling every platform binary inside a single VSIX
- Building the CLI from source inside the extension
- Editing user shell rc files
- Supporting unsupported architectures through ad hoc fallbacks
- Runtime selection of arbitrary CLI versions unrelated to the extension version

## Recommended Immediate Next Steps

1. Implement the binary manager abstraction in `packages/extension`.
2. Extend the CLI release workflow to publish a checksum manifest asset.
3. Add extension publish-time validation that the matching CLI release exists.
4. Update extension command/provider code to use absolute-path execution.
5. Rewrite extension tests around managed binary installation rather than ambient `PATH`.
6. Decide whether remote extension hosts are in or out for the first shipped version.

## Source References

These are the repo files this plan is based on:

- `packages/cli/package.json`
- `packages/cli/scripts/postinstall.js`
- `packages/cli/bin/wiki`
- `packages/extension/package.json`
- `packages/extension/src/utils/wikiBinary.ts`
- `packages/extension/src/commands/wikiQuickPick.ts`
- `packages/extension/src/providers/WikiEditorProvider.ts`
- `packages/extension/src/extension.ts`
- `packages/extension/test/runTest.ts`
- `.github/workflows/release-cli.yml`
- `.github/workflows/release-extension.yml`
