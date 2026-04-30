//! Per-mesh hint catalog and conservative anti-pattern detectors.
//!
//! Hints are structured variants — renderers stringify them. This module owns
//! detection only; the catalog itself is the contract for what comments can
//! appear above a `git mesh add` block.

use std::sync::OnceLock;

use regex::Regex;

/// Reasons the slug fell back to a non-heading source. Tracked so the renderer
/// can name the cause in a `# TODO: rename` comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FallbackReason {
    /// No section heading above the link; slug noun came from the link label.
    NoHeadingUsedLabel,
    /// No section heading and no link label; slug noun came from the target file stem.
    NoHeadingUsedFileStem,
}

/// Specific anti-pattern detected in the section opening sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AntiPattern {
    /// Opening tokens are a backticked identifier followed by an em-dash or "is".
    HeadlessPredicate,
    /// Opening matches `the X wiki section describes` (case-insensitive).
    CouplingTemplate,
    /// First non-stopword token is a verb from the verb-leading set.
    VerbLead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Hint {
    /// `count` siblings on the same page shared this anchor set and were merged.
    Consolidated { count: usize },
    /// This mesh shares a heading and target file with another mesh; cite it.
    ConsiderMerge { other_slug: String },
    /// Slug noun came from a fallback source.
    FallbackSlug { reason: FallbackReason },
    /// The section's opening sentence matches a known anti-pattern.
    WarnAntiPattern { pattern: AntiPattern },
}

/// Inspect a section opening sentence for any of the three configured
/// anti-patterns. Returns the *first* match, or `None`.
///
/// Conservative by design: each detector earns its keep with explicit positive
/// AND negative unit tests. False positives are worse than false negatives —
/// the reviewer reads the source sentence anyway.
pub(crate) fn detect_anti_patterns(section_opening: &str) -> Option<Hint> {
    let s = section_opening.trim();
    if s.is_empty() {
        return None;
    }
    if is_headless_predicate(s) {
        return Some(Hint::WarnAntiPattern {
            pattern: AntiPattern::HeadlessPredicate,
        });
    }
    if is_coupling_template(s) {
        return Some(Hint::WarnAntiPattern {
            pattern: AntiPattern::CouplingTemplate,
        });
    }
    if is_verb_lead(s) {
        return Some(Hint::WarnAntiPattern {
            pattern: AntiPattern::VerbLead,
        });
    }
    None
}

// ── Detectors ────────────────────────────────────────────────────────────────

fn headless_predicate_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // An identifier-shaped first token (originally backticked but stripped by
    // the prose cleaner) followed by ` — ` or ` is `. The leading-article
    // filter in `is_headless_predicate` keeps "The X is …" / "It is …" out.
    RE.get_or_init(|| {
        Regex::new(r"^`?([A-Za-z_][\w:]*)`?\s+(?:—|is)\s+").expect("valid regex")
    })
}

const HEADLESS_FIRST_TOKEN_DENYLIST: &[&str] = &[
    "the", "a", "an", "this", "that", "these", "those", "it", "its", "we",
    "they", "i", "he", "she", "there", "here", "use", "see",
];

fn coupling_template_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\bthe\s+\S+\s+wiki\s+section\s+describes\b").expect("valid regex")
    })
}

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "this", "that", "these", "those", "it", "its",
];

const LEADING_VERBS: &[&str] = &[
    "describes",
    "validates",
    "handles",
    "lives",
    "stores",
    "routes",
];

fn is_headless_predicate(s: &str) -> bool {
    let caps = match headless_predicate_re().captures(s) {
        Some(c) => c,
        None => return false,
    };
    let first = caps.get(1).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
    !HEADLESS_FIRST_TOKEN_DENYLIST.contains(&first.as_str())
}

fn is_coupling_template(s: &str) -> bool {
    coupling_template_re().is_match(s)
}

fn is_verb_lead(s: &str) -> bool {
    let tokens = s.split_whitespace();
    for tok in tokens {
        let lower = tok.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        if lower.is_empty() {
            continue;
        }
        if STOPWORDS.contains(&lower.as_str()) {
            continue;
        }
        return LEADING_VERBS.contains(&lower.as_str());
    }
    false
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_predicate_positive_with_backticks() {
        let h = detect_anti_patterns("`Foo` — does the thing.");
        assert!(matches!(
            h,
            Some(Hint::WarnAntiPattern { pattern: AntiPattern::HeadlessPredicate })
        ));
    }

    #[test]
    fn headless_predicate_positive_with_is() {
        let h = detect_anti_patterns("Foo is the thing that does X.");
        assert!(matches!(
            h,
            Some(Hint::WarnAntiPattern { pattern: AntiPattern::HeadlessPredicate })
        ));
    }

    #[test]
    fn headless_predicate_negative_normal_sentence() {
        // "The billing service validates …" — has a real subject, not a
        // headless identifier. Detector must not fire.
        assert!(detect_anti_patterns("The billing service validates the payload.").is_none());
    }

    #[test]
    fn coupling_template_positive() {
        let h = detect_anti_patterns(
            "The cache wiki section describes the LRU cache used by index lookups.",
        );
        assert!(matches!(
            h,
            Some(Hint::WarnAntiPattern { pattern: AntiPattern::CouplingTemplate })
        ));
    }

    #[test]
    fn coupling_template_negative_partial_phrase() {
        // Has "wiki" and "section" and "describes" but not in the trigger order
        // for the coupling template, and the leading subject is a noun (no
        // verb_lead).
        assert!(
            detect_anti_patterns("Our handler reads the wiki section before invoking validate.")
                .is_none()
        );
    }

    #[test]
    fn verb_lead_positive() {
        let h = detect_anti_patterns("Validates the request payload before dispatch.");
        assert!(matches!(
            h,
            Some(Hint::WarnAntiPattern { pattern: AntiPattern::VerbLead })
        ));
    }

    #[test]
    fn verb_lead_positive_after_stopword() {
        let h = detect_anti_patterns("The describes flow runs first.");
        // First non-stopword "describes" — fires.
        assert!(matches!(
            h,
            Some(Hint::WarnAntiPattern { pattern: AntiPattern::VerbLead })
        ));
    }

    #[test]
    fn verb_lead_negative_noun_subject() {
        assert!(detect_anti_patterns("The handler accepts a request.").is_none());
    }

    #[test]
    fn empty_returns_none() {
        assert!(detect_anti_patterns("").is_none());
        assert!(detect_anti_patterns("   ").is_none());
    }
}
