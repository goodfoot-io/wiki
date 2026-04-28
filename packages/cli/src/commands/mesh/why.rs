//! Why-generation utilities ported from `scripts/mesh-scaffold-v4.mjs`.
//!
//! Phase B implements only the template-why fallback. The prose-why extractor
//! is stubbed and returns `None`, so all whys come from the template branches.
//! Phase C will port `extractProseWhy` in full.

use crate::parser::FragmentLink;

use super::name::{cap, norm_cmp};
use super::words::RelType;

/// Phase B stub — Phase C will replace this with the JS `extractProseWhy` port.
///
/// Returns `None` unconditionally so the caller falls back to [`template_why`].
#[allow(dead_code, unused_variables)]
pub(crate) fn extract_prose_why(link: &FragmentLink) -> Option<String> {
    None
}

/// Port of the JS `templateWhy` dispatch: each rel-type maps to a closure that
/// composes the why string from the four input phrases.
pub(crate) fn template_why(
    rel: &RelType,
    core_phrase: &str,
    object_phrase: &str,
    source_role: &str,
    target_role: &str,
) -> String {
    let core = core_phrase;
    let obj = object_phrase;
    let src = source_role;
    let tgt = target_role;
    let core_eq_tgt = norm_cmp(core) == norm_cmp(tgt);
    let obj_eq_tgt = norm_cmp(obj) == norm_cmp(tgt);
    let core_eq_obj = norm_cmp(core) == norm_cmp(obj);

    match rel.rel_type {
        "contract" => {
            if obj_eq_tgt || core_eq_obj {
                format!(
                    "{} data contract in {tgt}, as specified in the {src} wiki section.",
                    cap(core)
                )
            } else {
                let shape_word = if obj.to_lowercase().contains("shape") {
                    "structure"
                } else {
                    "shape"
                };
                format!(
                    "{} contract that synchronizes the {obj} {shape_word} expected by the {src} wiki section with what {tgt} provides.",
                    cap(core)
                )
            }
        }
        "rule" => {
            if obj_eq_tgt {
                format!(
                    "{} enforcement rule in {tgt}, as specified in the {src} wiki section.",
                    cap(core)
                )
            } else {
                format!(
                    "{} enforcement rule shared between the {src} wiki section and the {tgt} implementation.",
                    cap(core)
                )
            }
        }
        "flow" => {
            if obj_eq_tgt {
                format!(
                    "{} flow in {tgt}, as described in the {src} wiki section.",
                    cap(core)
                )
            } else {
                format!(
                    "{} flow that routes {obj} as documented in the {src} wiki section and implemented in {tgt}.",
                    cap(core)
                )
            }
        }
        "config" => {
            if obj_eq_tgt {
                format!(
                    "{} configuration in {tgt}, as specified in the {src} wiki section.",
                    cap(core)
                )
            } else {
                format!(
                    "{} configuration that the {src} wiki section specifies and {tgt} consumes.",
                    cap(core)
                )
            }
        }
        // Default: sync.
        _ => {
            if core_eq_tgt {
                format!("{} — covered by the {src} wiki section.", cap(core))
            } else if obj_eq_tgt {
                format!("{} — the {src} wiki section describes {tgt}.", cap(core))
            } else {
                format!(
                    "{} — the {src} wiki section describes {obj} in {tgt}.",
                    cap(core)
                )
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::words::REL_TYPES;
    use super::*;

    fn rel(name: &str) -> &'static RelType {
        REL_TYPES.iter().find(|r| r.rel_type == name).expect("rel")
    }

    #[test]
    fn sync_default_three_part() {
        let w = template_why(
            rel("sync"),
            "core thing",
            "object thing",
            "src section",
            "tgt impl",
        );
        assert_eq!(
            w,
            "Core thing — the src section wiki section describes object thing in tgt impl."
        );
    }

    #[test]
    fn sync_obj_equals_tgt_two_part() {
        let w = template_why(rel("sync"), "core", "tgt", "src", "tgt");
        assert_eq!(w, "Core — the src wiki section describes tgt.");
    }

    #[test]
    fn sync_core_equals_tgt_collapses() {
        let w = template_why(rel("sync"), "tgt", "obj", "src", "tgt");
        assert_eq!(w, "Tgt — covered by the src wiki section.");
    }

    #[test]
    fn contract_with_obj_eq_tgt_uses_tautology_form() {
        let w = template_why(rel("contract"), "checkout", "tgt", "billing", "tgt");
        assert_eq!(
            w,
            "Checkout data contract in tgt, as specified in the billing wiki section."
        );
    }

    #[test]
    fn contract_general_form() {
        let w = template_why(rel("contract"), "checkout", "payload", "billing", "stripe");
        assert_eq!(
            w,
            "Checkout contract that synchronizes the payload shape expected by the billing wiki section with what stripe provides."
        );
    }

    #[test]
    fn contract_avoids_shape_shape() {
        let w = template_why(
            rel("contract"),
            "checkout",
            "the shape",
            "billing",
            "stripe",
        );
        assert!(w.contains("the shape structure"));
        assert!(!w.contains("shape shape"));
    }

    #[test]
    fn rule_obj_eq_tgt_collapses() {
        let w = template_why(rel("rule"), "permission", "tgt", "auth", "tgt");
        assert_eq!(
            w,
            "Permission enforcement rule in tgt, as specified in the auth wiki section."
        );
    }

    #[test]
    fn rule_general_form() {
        let w = template_why(rel("rule"), "permission", "user", "auth", "guard");
        assert_eq!(
            w,
            "Permission enforcement rule shared between the auth wiki section and the guard implementation."
        );
    }

    #[test]
    fn flow_obj_eq_tgt_collapses() {
        let w = template_why(rel("flow"), "request", "tgt", "api", "tgt");
        assert_eq!(
            w,
            "Request flow in tgt, as described in the api wiki section."
        );
    }

    #[test]
    fn flow_general_form() {
        let w = template_why(rel("flow"), "request", "payload", "api", "handler");
        assert_eq!(
            w,
            "Request flow that routes payload as documented in the api wiki section and implemented in handler."
        );
    }

    #[test]
    fn config_obj_eq_tgt_collapses() {
        let w = template_why(rel("config"), "env", "tgt", "deploy", "tgt");
        assert_eq!(
            w,
            "Env configuration in tgt, as specified in the deploy wiki section."
        );
    }

    #[test]
    fn config_general_form() {
        let w = template_why(rel("config"), "env", "secret", "deploy", "wrangler");
        assert_eq!(
            w,
            "Env configuration that the deploy wiki section specifies and wrangler consumes."
        );
    }

    #[test]
    fn extract_prose_why_is_phase_b_stub() {
        let link = FragmentLink {
            kind: crate::parser::LinkKind::Internal,
            path: "x.rs".into(),
            start_line: Some(1),
            end_line: Some(2),
            text: "x".into(),
            original_text: "x".into(),
            original_href: "x.rs#L1-L2".into(),
            source_line: 1,
        };
        assert_eq!(extract_prose_why(&link), None);
    }
}
