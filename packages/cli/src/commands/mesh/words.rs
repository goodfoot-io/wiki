/// File extensions used as stop tokens during tokenization.
///
/// Ported verbatim from `scripts/mesh-scaffold-v4.mjs` `FILE_EXTS`.
/// Sorted for `binary_search` lookup.
pub(crate) const FILE_EXTS: &[&str] = &[
    "bash", "cjs", "css", "env", "go", "html", "js", "json", "jsx", "md", "mdx", "mjs", "py", "rb",
    "rs", "scss", "sh", "swift", "toml", "ts", "tsx", "yaml", "yml",
];

/// Stop words — high-frequency function words and programming keywords.
///
/// Ported verbatim from `scripts/mesh-scaffold-v4.mjs` `STOP`.
/// Sorted for `binary_search` lookup.
pub(crate) const STOP: &[&str] = &[
    "a",
    "above",
    "across",
    "after",
    "all",
    "also",
    "always",
    "an",
    "and",
    "any",
    "are",
    "as",
    "async",
    "at",
    "await",
    "be",
    "before",
    "below",
    "between",
    "both",
    "by",
    "can",
    "class",
    "const",
    "correctly",
    "currently",
    "do",
    "does",
    "each",
    "either",
    "else",
    "enum",
    "every",
    "exactly",
    "export",
    "false",
    "for",
    "from",
    "function",
    "get",
    "has",
    "have",
    "here",
    "how",
    "if",
    "import",
    "in",
    "initially",
    "inside",
    "interface",
    "into",
    "is",
    "it",
    "its",
    "just",
    "less",
    "let",
    "more",
    "must",
    "neither",
    "never",
    "new",
    "not",
    "null",
    "of",
    "often",
    "old",
    "on",
    "only",
    "or",
    "outside",
    "private",
    "properly",
    "protected",
    "public",
    "readonly",
    "return",
    "same",
    "set",
    "should",
    "simply",
    "some",
    "static",
    "still",
    "strictly",
    "that",
    "the",
    "their",
    "then",
    "there",
    "these",
    "this",
    "those",
    "through",
    "to",
    "true",
    "type",
    "undefined",
    "use",
    "used",
    "using",
    "var",
    "via",
    "what",
    "when",
    "where",
    "which",
    "who",
    "why",
    "will",
    "with",
    "within",
    "without",
];

/// Noise words — structural, generic, and path labels never used as subsystem names.
///
/// Ported verbatim from `scripts/mesh-scaffold-v4.mjs` `NOISE`.
/// Sorted for `binary_search` lookup.
pub(crate) const NOISE: &[&str] = &[
    "api",
    "app",
    "apps",
    "barrel",
    "base",
    "client",
    "common",
    "component",
    "components",
    "controller",
    "controllers",
    "data",
    "dependencies",
    "dependency",
    "deps",
    "detail",
    "details",
    "doc",
    "docs",
    "documentation",
    "entries",
    "entry",
    "example",
    "examples",
    "file",
    "global",
    "handler",
    "handlers",
    "helper",
    "helpers",
    "impl",
    "implementation",
    "index",
    "input",
    "item",
    "items",
    "lib",
    "link",
    "links",
    "main",
    "middleware",
    "mock",
    "mocks",
    "model",
    "models",
    "note",
    "notes",
    "output",
    "packages",
    "page",
    "piece",
    "pkg",
    "readme",
    "reference",
    "references",
    "result",
    "results",
    "root",
    "route",
    "routes",
    "sample",
    "samples",
    "schema",
    "schemas",
    "server",
    "service",
    "services",
    "shared",
    "snippet",
    "source",
    "sources",
    "spec",
    "src",
    "test",
    "tests",
    "type",
    "types",
    "util",
    "utils",
    "value",
    "values",
    "www",
];

/// Token sets for relationship-type detection.
///
/// Ported verbatim from `scripts/mesh-scaffold-v4.mjs` `REL_TYPES`.
/// Each variant's `words` slice is sorted for `binary_search` lookup.
pub(crate) struct RelType {
    pub(crate) rel_type: &'static str,
    pub(crate) threshold: usize,
    pub(crate) words: &'static [&'static str],
}

pub(crate) const REL_TYPES: &[RelType] = &[
    RelType {
        rel_type: "contract",
        threshold: 2,
        words: &[
            "contract",
            "deserializes",
            "expects",
            "format",
            "matches",
            "parses",
            "payload",
            "schema",
            "serializes",
            "shape",
            "structure",
            "validates",
        ],
    },
    RelType {
        rel_type: "rule",
        threshold: 2,
        words: &[
            "allowed",
            "boundary",
            "constraint",
            "denies",
            "enforces",
            "forbidden",
            "governs",
            "guard",
            "invariant",
            "permission",
            "policy",
            "rejects",
            "validates",
        ],
    },
    RelType {
        rel_type: "flow",
        threshold: 3,
        words: &[
            "dispatches",
            "emits",
            "endpoint",
            "handler",
            "pipeline",
            "propagates",
            "request",
            "response",
            "routes",
            "submits",
            "subscribes",
            "triggers",
            "webhook",
        ],
    },
    RelType {
        rel_type: "config",
        threshold: 2,
        words: &[
            "binding",
            "config",
            "configuration",
            "deploy",
            "env",
            "environment",
            "flag",
            "option",
            "secret",
            "settings",
            "variable",
            "wrangler",
        ],
    },
    RelType {
        rel_type: "sync",
        threshold: 0,
        words: &[],
    },
];

/// Category detection token sets.
///
/// Ported verbatim from `scripts/mesh-scaffold-v4.mjs` `CATEGORIES`.
/// Each category's `words` slice is sorted for `binary_search` lookup.
pub(crate) struct Category {
    pub(crate) name: &'static str,
    pub(crate) words: &'static [&'static str],
}

pub(crate) const CATEGORIES: &[Category] = &[
    Category {
        name: "billing",
        words: &[
            "billing",
            "charge",
            "checkout",
            "invoice",
            "payment",
            "payments",
            "stripe",
            "subscription",
        ],
    },
    Category {
        name: "auth",
        words: &[
            "auth",
            "authentication",
            "authkit",
            "authorization",
            "jwt",
            "login",
            "logout",
            "oauth",
            "session",
            "token",
            "workos",
        ],
    },
    Category {
        name: "experiments",
        words: &[
            "abtest",
            "bucket",
            "experiment",
            "feature",
            "flag",
            "rollout",
            "treatment",
            "variant",
        ],
    },
    Category {
        name: "platform",
        words: &[
            "build",
            "ci",
            "deploy",
            "deployment",
            "infra",
            "infrastructure",
            "logging",
            "metrics",
            "observability",
            "platform",
        ],
    },
    Category {
        name: "data",
        words: &[
            "analytics",
            "data",
            "database",
            "etl",
            "event",
            "migration",
            "sync",
            "warehouse",
        ],
    },
    Category {
        name: "security",
        words: &[
            "compliance",
            "control",
            "mitigation",
            "permission",
            "policy",
            "risk",
            "security",
            "threat",
        ],
    },
    Category {
        name: "notifications",
        words: &[
            "email",
            "message",
            "notification",
            "sms",
            "template",
            "webhook",
        ],
    },
    Category {
        name: "cli",
        words: &[
            "cli", "command", "flag", "option", "parser", "repl", "stdin", "stdout",
        ],
    },
];

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_exts_is_sorted() {
        assert!(
            FILE_EXTS.is_sorted(),
            "FILE_EXTS must be sorted for binary_search"
        );
    }

    #[test]
    fn stop_is_sorted() {
        assert!(STOP.is_sorted(), "STOP must be sorted for binary_search");
    }

    #[test]
    fn noise_is_sorted() {
        assert!(NOISE.is_sorted(), "NOISE must be sorted for binary_search");
    }

    #[test]
    fn rel_type_words_are_sorted() {
        for rel in REL_TYPES {
            assert!(
                rel.words.is_sorted(),
                "REL_TYPES[{}].words must be sorted for binary_search",
                rel.rel_type
            );
        }
    }

    #[test]
    fn category_words_are_sorted() {
        for cat in CATEGORIES {
            assert!(
                cat.words.is_sorted(),
                "CATEGORIES[{}].words must be sorted for binary_search",
                cat.name
            );
        }
    }
}
