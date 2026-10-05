/**
 * Environment variables the extension and its test harness read or write.
 *
 * Declaring them on `NodeJS.ProcessEnv` turns `process.env.NAME` into a typed
 * property access, which satisfies both `noPropertyAccessFromIndexSignature`
 * and Biome's `useLiteralKeys` without bracket access.
 *
 * @summary Typed declarations for the process environment variables used by this package.
 */

declare namespace NodeJS {
  interface ProcessEnv {
    /** Executable search path; read for binary lookup, extended by the test harness. */
    PATH?: string;
    /** Windows executable extensions consulted during binary lookup. */
    PATHEXT?: string;
    /** Overrides the base URL the wiki CLI installer downloads releases from. */
    WIKI_EXTENSION_RELEASE_BASE_URL?: string;
    /** `1` lets the extension fall back to a `wiki` binary found on PATH. */
    WIKI_EXTENSION_USE_PATH_FALLBACK?: string;
    /** Set by CI providers; selects headless test mode. */
    CI?: string;
    /** Set by GitHub Actions; selects headless test mode. */
    GITHUB_ACTIONS?: string;
    /** Forces headless test mode; also exported by the harness. */
    HEADLESS?: string;
    /** X11 display; the harness starts Xvfb and sets it when absent. */
    DISPLAY?: string;
    /** Exported by the test harness to run Electron without its sandbox. */
    ELECTRON_DISABLE_SANDBOX?: string;
    /** Exported by the test harness to run Electron without its GPU sandbox. */
    ELECTRON_DISABLE_GPU_SANDBOX?: string;
    /** Exported by the test harness to silence Electron security warnings. */
    ELECTRON_DISABLE_SECURITY_WARNINGS?: string;
    /** Restricts the test run to suites matching this pattern. */
    TEST_PATTERN?: string;
  }
}
