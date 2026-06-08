//! Advisory rendering for the mesh-coverage engine.
//!
//! Renders the human-readable advisory block (parse errors, dropped meshes)
//! and blocker-rename lines that `wiki check --fix` routes to stderr.

use super::scaffold::{DropReason, DroppedMesh, ParseError, PlannedRename};

/// Render the advisory block (parse errors and dropped meshes).
///
/// `has_scaffold_following` controls the parse-error header phrasing:
/// - `true`  → advisory ("Some wiki pages could not be parsed and were skipped:")
/// - `false` → hard-stop ("Unable to generate scaffolding due to parsing errors:")
pub(crate) fn render_advisories(
    out: &mut String,
    parse_errors: &[ParseError],
    dropped_meshes: &[DroppedMesh],
    has_scaffold_following: bool,
) {
    use std::fmt::Write as _;
    if !parse_errors.is_empty() {
        let header = if has_scaffold_following {
            "Some wiki pages could not be parsed and were skipped:"
        } else {
            "Unable to generate scaffolding due to parsing errors:"
        };
        let _ = writeln!(out, "{header}");
        for e in parse_errors {
            let _ = writeln!(out, "- {} ({})", e.path, e.kind.reason());
        }
        out.push('\n');
    }
    for d in dropped_meshes {
        match &d.reason {
            DropReason::MissingPath { path } => {
                let _ = writeln!(
                    out,
                    "Skipped mesh `{}` — references missing path `{}` (page `{}`). \
                     Once the path exists, anchor BOTH the page and the code target:\n  \
                     wiki mesh add {} {} {} --why \"<rationale>\"",
                    d.slug, path, d.page, d.slug, d.page, path
                );
            }
            DropReason::IgnoredPath { path } => {
                let _ = writeln!(
                    out,
                    "Skipped gitignored anchor `{}` in mesh `{}` (page `{}`); \
                     a path not tracked by git cannot be anchored. Anchor a tracked \
                     code target alongside the page:\n  \
                     wiki mesh add {} {} <tracked-code-anchor> --why \"<rationale>\"",
                    path, d.slug, d.page, d.slug, d.page
                );
            }
            DropReason::InvalidAnchor { anchor, detail } => {
                let _ = writeln!(
                    out,
                    "Skipped mesh `{}` — invalid anchor `{}` ({}) (page `{}`). \
                     Re-add with a valid in-bounds anchor covering BOTH the page and \
                     the code target:\n  \
                     wiki mesh add {} {} <valid-code-anchor> --why \"<rationale>\"",
                    d.slug, anchor, detail, d.page, d.slug, d.page
                );
            }
            DropReason::SlugPathCollision { existing } => {
                let _ = writeln!(
                    out,
                    "Skipped mesh `{}` — slug path collides with existing mesh `{}` (page `{}`). \
                     Prefer renaming this draft's slug so it no longer path-collides with `{}`, \
                     then re-run `wiki check --fix`. Only if `{}` is itself stale should you \
                     remove it — this DELETES its anchors and `why` and may raise a fresh \
                     mesh_uncovered failure for whatever it covered:\n  \
                     wiki mesh remove {}",
                    d.slug, existing, d.page, existing, existing, existing
                );
            }
        }
    }
    if !dropped_meshes.is_empty() {
        out.push('\n');
    }
}

/// Render the planned/performed blocker-rename advisory lines.
///
/// `dry_run` selects the phrasing:
/// - `true`  → "Would rename mesh `B` -> `B/target` to free path for `S`."
/// - `false` → "Renamed mesh `B` -> `B/target` to free path for `S` (page `P`)."
pub(crate) fn render_rename_advisories(out: &mut String, renames: &[PlannedRename], dry_run: bool) {
    use std::fmt::Write as _;
    if renames.is_empty() {
        return;
    }
    for r in renames {
        if dry_run {
            let _ = writeln!(
                out,
                "Would rename mesh `{}` -> `{}` to free path for `{}`.",
                r.from, r.to, r.for_slug
            );
        } else {
            let _ = writeln!(
                out,
                "Renamed mesh `{}` -> `{}` to free path for `{}` (page `{}`).",
                r.from, r.to, r.for_slug, r.page
            );
        }
    }
    out.push('\n');
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::scaffold::{DropReason, ParseErrorKind};
    use super::*;

    fn make_error(path: &str, kind: ParseErrorKind) -> ParseError {
        ParseError {
            path: path.to_string(),
            kind,
        }
    }

    fn make_dropped(slug: &str, reason: DropReason, page: &str) -> DroppedMesh {
        DroppedMesh {
            slug: slug.to_string(),
            reason,
            page: page.to_string(),
        }
    }

    // ── advisory block ─────────────────────────────────────────────────────────

    #[test]
    fn render_advisories_uses_advisory_header_when_content_follows() {
        let mut out = String::new();
        let errors = vec![make_error("wiki/bad.md", ParseErrorKind::EmptyTitle)];
        render_advisories(&mut out, &errors, &[], true);
        assert!(
            out.starts_with("Some wiki pages could not be parsed and were skipped:\n"),
            "expected advisory header, got:\n{out}"
        );
        assert!(out.contains("wiki/bad.md (frontmatter present but `title:` is empty)"));
    }

    #[test]
    fn render_advisories_uses_hard_stop_header_when_nothing_follows() {
        let mut out = String::new();
        let errors = vec![make_error("wiki/bad.md", ParseErrorKind::EmptyTitle)];
        render_advisories(&mut out, &errors, &[], false);
        assert!(
            out.starts_with("Unable to generate scaffolding due to parsing errors:\n"),
            "expected hard-stop header, got:\n{out}"
        );
    }

    #[test]
    fn render_advisories_names_dropped_missing_path() {
        let mut out = String::new();
        let dropped = vec![make_dropped(
            "wiki/foo",
            DropReason::MissingPath {
                path: "src/missing.rs".to_string(),
            },
            "wiki/page.md",
        )];
        render_advisories(&mut out, &[], &dropped, true);
        assert!(
            out.contains("Skipped mesh `wiki/foo`")
                && out.contains("src/missing.rs")
                && out.contains("wiki/page.md"),
            "missing-path advisory must name slug, path, page:\n{out}"
        );
        // F3: a runnable command anchoring BOTH the page and the code target.
        assert!(
            out.contains("wiki mesh add wiki/foo wiki/page.md src/missing.rs --why"),
            "missing-path advisory must emit a runnable add command anchoring both:\n{out}"
        );
    }

    #[test]
    fn render_advisories_ignored_path_names_command() {
        let mut out = String::new();
        let dropped = vec![make_dropped(
            "wiki/foo",
            DropReason::IgnoredPath {
                path: "target/gen.rs".to_string(),
            },
            "wiki/page.md",
        )];
        render_advisories(&mut out, &[], &dropped, true);
        assert!(
            out.contains("wiki mesh add wiki/foo wiki/page.md <tracked-code-anchor> --why"),
            "ignored-path advisory must emit a runnable add command:\n{out}"
        );
    }

    #[test]
    fn render_advisories_invalid_anchor_names_command() {
        let mut out = String::new();
        let dropped = vec![make_dropped(
            "wiki/foo",
            DropReason::InvalidAnchor {
                anchor: "src/a.rs#L9-L99".to_string(),
                detail: "out of bounds".to_string(),
            },
            "wiki/page.md",
        )];
        render_advisories(&mut out, &[], &dropped, true);
        assert!(
            out.contains("wiki mesh add wiki/foo wiki/page.md <valid-code-anchor> --why"),
            "invalid-anchor advisory must emit a runnable add command:\n{out}"
        );
    }

    #[test]
    fn render_advisories_slug_collision_names_command() {
        let mut out = String::new();
        let dropped = vec![make_dropped(
            "wiki/foo/helper",
            DropReason::SlugPathCollision {
                existing: "wiki/foo".to_string(),
            },
            "wiki/page.md",
        )];
        render_advisories(&mut out, &[], &dropped, true);
        assert!(
            out.contains("wiki mesh remove wiki/foo"),
            "slug-collision advisory must emit a runnable remove command:\n{out}"
        );
    }

    #[test]
    fn render_advisories_empty_when_no_errors_or_drops() {
        let mut out = String::new();
        render_advisories(&mut out, &[], &[], true);
        assert!(out.is_empty(), "expected empty advisory block, got:\n{out}");
    }

    #[test]
    fn render_parse_error_reason_strings() {
        fn reason(kind: ParseErrorKind) -> String {
            kind.reason()
        }
        assert_eq!(
            reason(ParseErrorKind::EmptyTitle),
            "frontmatter present but `title:` is empty"
        );
        assert_eq!(
            reason(ParseErrorKind::Unreadable("oops".to_string())),
            "file could not be read: oops"
        );
        assert_eq!(
            reason(ParseErrorKind::Malformed),
            "malformed frontmatter — could not parse `title`"
        );
    }

    // ── rename advisories ────────────────────────────────────────────────────

    #[test]
    fn render_rename_advisories_phrasing_follows_dry_run() {
        let renames = vec![PlannedRename {
            from: "wiki/arch/scaff".to_string(),
            to: "wiki/arch/scaff/index".to_string(),
            for_slug: "wiki/arch/scaff/helper".to_string(),
            page: "wiki/arch/scaff/page.md".to_string(),
        }];

        let mut dry = String::new();
        render_rename_advisories(&mut dry, &renames, true);
        assert!(
            dry.contains("Would rename mesh `wiki/arch/scaff` -> `wiki/arch/scaff/index`"),
            "dry-run phrasing missing:\n{dry}"
        );

        let mut applied = String::new();
        render_rename_advisories(&mut applied, &renames, false);
        assert!(
            applied.contains("Renamed mesh `wiki/arch/scaff` -> `wiki/arch/scaff/index`"),
            "applied phrasing missing:\n{applied}"
        );
    }
}
