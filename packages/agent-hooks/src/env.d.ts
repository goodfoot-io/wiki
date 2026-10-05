/**
 * The environment variables agent-hooks sources, tests and scripts read or
 * write, declared so they are accessed as typed properties rather than through
 * `process.env`'s string index signature.
 */
declare namespace NodeJS {
  interface ProcessEnv {
    /** Absolute path override for the `wiki` binary the hooks spawn. */
    WIKI_BIN?: string;
    /** POSIX home directory; `os.homedir()` reads it, so tests redirect it to a fixture. */
    HOME?: string;
    /** Windows roaming application-data root; locates VS Code's globalStorage. */
    APPDATA?: string;
    /** Git repository-context variables the layout tests strip from child processes. */
    GIT_DIR?: string;
    GIT_WORK_TREE?: string;
    GIT_INDEX_FILE?: string;
    GIT_OBJECT_DIRECTORY?: string;
  }
}
