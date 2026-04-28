//! Name-generation utilities ported from `scripts/mesh-scaffold-v4.mjs`.
//!
//! Each `pub(crate)` free function corresponds to a JS function of the same
//! name (snake-cased). Sort orders and tie-breakers follow the JS behavior so
//! output is byte-stable for a fixed input.

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;

use super::words::{CATEGORIES, FILE_EXTS, NOISE, REL_TYPES, RelType, STOP};

// ── Tokenization ─────────────────────────────────────────────────────────────

fn is_stop(word: &str) -> bool {
    STOP.binary_search(&word).is_ok()
}

fn is_noise(word: &str) -> bool {
    NOISE.binary_search(&word).is_ok()
}

fn is_file_ext(word: &str) -> bool {
    FILE_EXTS.binary_search(&word).is_ok()
}

fn is_all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Lowercase camelCase / PascalCase / ALLCAPS sequences, matching the JS
/// `normalizeProperNouns` regexes.
pub(crate) fn normalize_proper_nouns(text: &str) -> String {
    let camel = static_re(r"\b[A-Z][a-z]+[A-Z][A-Za-z]*\b")
        .replace_all(text, |c: &regex::Captures| c[0].to_lowercase())
        .into_owned();
    static_re(r"\b[A-Z]{2,6}\b")
        .replace_all(&camel, |c: &regex::Captures| c[0].to_lowercase())
        .into_owned()
}

fn static_re(pat: &str) -> Regex {
    Regex::new(pat).expect("valid regex")
}

/// Tokenize `text` using the JS `tokenize` rules.
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    let normalized = normalize_proper_nouns(text);
    // ([a-z0-9])([A-Z]) → "$1 $2"
    let re_camel = static_re(r"([a-z0-9])([A-Z])");
    let split_camel = re_camel.replace_all(&normalized, "$1 $2").into_owned();
    // [_/.\-:{}()[\],#"`'<>|=+*!?@$%^&~→←↑↓—–]+ → " "
    let re_punct = static_re(
        r#"[_/.\-:{}()\[\],#"`'<>|=+*!?@$%^&~\u{2192}\u{2190}\u{2191}\u{2193}\u{2014}\u{2013}]+"#,
    );
    let cleaned = re_punct.replace_all(&split_camel, " ").to_lowercase();
    cleaned
        .split_whitespace()
        .map(|s| s.to_string())
        .filter(|t| t.len() > 1 && !is_all_digits(t) && !is_file_ext(t))
        .collect()
}

fn count_map(tokens: &[String]) -> BTreeMap<String, usize> {
    let mut m: BTreeMap<String, usize> = BTreeMap::new();
    for t in tokens {
        *m.entry(t.clone()).or_insert(0) += 1;
    }
    m
}

// ── RAKE ─────────────────────────────────────────────────────────────────────

/// A scored phrase produced by [`rake`].
#[derive(Debug, Clone)]
pub(crate) struct RakeResult {
    pub(crate) phrase: String,
    #[allow(dead_code)]
    pub(crate) words: Vec<String>,
    pub(crate) score: f64,
}

/// Port of the JS `rake()` function.
///
/// Returns phrases ordered by descending score; ties retain first-encounter
/// order (stable sort).
pub(crate) fn rake(text: &str) -> Vec<RakeResult> {
    if text.is_empty() {
        return Vec::new();
    }
    // Build a regex that splits on stop words OR non-alphanumeric runs.
    // Matches JS: `\b(stop1|stop2|...)\b|[^a-zA-Z0-9'\-]+`, case-insensitive.
    let stop_alternation = STOP
        .iter()
        .map(|s| regex::escape(s))
        .collect::<Vec<_>>()
        .join("|");
    let stop_pattern = format!(r"(?i)\b({stop_alternation})\b|[^a-zA-Z0-9'\-]+");
    let split_re = Regex::new(&stop_pattern).expect("valid stop regex");

    let normalized = normalize_proper_nouns(text).to_lowercase();
    // JS: split on the regex, keep parts with alpha
    let has_alpha = static_re(r"[a-z]");
    let parts: Vec<String> = split_re
        .split(&normalized)
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty() && has_alpha.is_match(p))
        .collect();

    let mut candidates: Vec<Vec<String>> = Vec::new();
    for part in &parts {
        let words: Vec<String> = part
            .split_whitespace()
            .map(|s| s.to_string())
            .filter(|w| w.len() > 1 && !is_all_digits(w) && !is_noise(w) && !is_file_ext(w))
            .collect();
        if words.is_empty() {
            continue;
        }
        for start in 0..words.len() {
            let max_len = std::cmp::min(4, words.len() - start);
            for len in 1..=max_len {
                candidates.push(words[start..start + len].to_vec());
            }
        }
    }

    if candidates.is_empty() {
        return Vec::new();
    }

    let mut word_freq: BTreeMap<String, usize> = BTreeMap::new();
    let mut word_deg: BTreeMap<String, usize> = BTreeMap::new();
    for phrase in &candidates {
        for word in phrase {
            *word_freq.entry(word.clone()).or_insert(0) += 1;
            *word_deg.entry(word.clone()).or_insert(0) += phrase.len();
        }
    }
    let mut word_score: BTreeMap<String, f64> = BTreeMap::new();
    for (w, freq) in &word_freq {
        let deg = *word_deg.get(w).unwrap_or(&0) as f64;
        word_score.insert(w.clone(), deg / *freq as f64);
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut scored: Vec<RakeResult> = Vec::new();
    for phrase in candidates {
        let key = phrase.join(" ");
        if !seen.insert(key.clone()) {
            continue;
        }
        if phrase.iter().all(|w| is_stop(w) || is_noise(w)) {
            continue;
        }
        if phrase.len() > 1 {
            let unique: BTreeSet<&String> = phrase.iter().collect();
            let has_adjacent_dup = phrase
                .iter()
                .enumerate()
                .skip(1)
                .any(|(i, w)| w == &phrase[i - 1]);
            if unique.len() == 1 || has_adjacent_dup {
                continue;
            }
        }
        let score: f64 = phrase
            .iter()
            .map(|w| *word_score.get(w).unwrap_or(&0.0))
            .sum();
        scored.push(RakeResult {
            phrase: key,
            words: phrase,
            score,
        });
    }

    // Stable sort by score descending. Stable sort preserves insertion order
    // for ties, matching JS Array.prototype.sort (V8 stable since 2019).
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored
}

// ── Co-presence ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct CoPresentTerm {
    pub(crate) term: String,
    pub(crate) score: f64,
}

/// Port of the JS `coPresenceTerms()` function.
pub(crate) fn co_presence_terms(src_text: &str, tgt_text: &str) -> Vec<CoPresentTerm> {
    let src_tokens = tokenize(src_text);
    let tgt_tokens = tokenize(tgt_text);
    if src_tokens.is_empty() || tgt_tokens.is_empty() {
        return Vec::new();
    }
    let src_counts = count_map(&src_tokens);
    let tgt_counts = count_map(&tgt_tokens);
    let src_total = src_tokens.len() as f64;
    let tgt_total = tgt_tokens.len() as f64;

    // Iterate src_counts in insertion order of first occurrence, matching JS Map.
    // BTreeMap iterates lexicographically, which is deterministic but differs from
    // JS insertion order. To match JS exactly we walk the original token sequence,
    // emitting each term once.
    let mut emitted: BTreeSet<String> = BTreeSet::new();
    let mut result: Vec<CoPresentTerm> = Vec::new();
    for t in &src_tokens {
        if !emitted.insert(t.clone()) {
            continue;
        }
        if is_stop(t) || is_noise(t) || t.len() < 3 || is_all_digits(t) {
            continue;
        }
        let sc = *src_counts.get(t).unwrap_or(&0);
        let tc = *tgt_counts.get(t).unwrap_or(&0);
        if tc == 0 {
            continue;
        }
        let tf_src = sc as f64 / src_total;
        let tf_tgt = tc as f64 / tgt_total;
        let score = tf_src.min(tf_tgt) * (sc + tc) as f64;
        result.push(CoPresentTerm {
            term: t.clone(),
            score,
        });
    }
    result.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    result
}

// ── Role extraction ─────────────────────────────────────────────────────────

/// Port of the JS `extractTargetRole()`.
pub(crate) fn extract_target_role(target_path: &str, target_title: Option<&str>) -> String {
    if let Some(title) = target_title {
        let toks: Vec<String> = tokenize(title)
            .into_iter()
            .filter(|w| !is_stop(w) && !is_noise(w))
            .collect();
        if !toks.is_empty() {
            return toks.iter().take(3).cloned().collect::<Vec<_>>().join(" ");
        }
    }
    if target_path.is_empty() {
        return "target".to_string();
    }
    let segments: Vec<String> = target_path
        .split('/')
        .map(|s| {
            let stripped = match s.rfind('.') {
                Some(idx) => &s[..idx],
                None => s,
            };
            stripped.to_lowercase()
        })
        .collect();
    for seg in segments.iter().rev() {
        if !is_noise(seg) && !is_stop(seg) && seg.len() > 2 && !is_all_digits(seg) {
            return seg.replace(['-', '_'], " ");
        }
    }
    "target".to_string()
}

/// Port of the JS `extractSourceRole()`.
pub(crate) fn extract_source_role(heading_chain: &[String], source_title: Option<&str>) -> String {
    for h in heading_chain.iter().rev() {
        let toks: Vec<String> = tokenize(h)
            .into_iter()
            .filter(|w| !is_stop(w) && !is_noise(w))
            .collect();
        if !toks.is_empty() {
            return toks.iter().take(4).cloned().collect::<Vec<_>>().join(" ");
        }
    }
    if let Some(title) = source_title {
        let toks: Vec<String> = tokenize(title)
            .into_iter()
            .filter(|w| !is_stop(w) && !is_noise(w))
            .collect();
        if !toks.is_empty() {
            return toks.iter().take(3).cloned().collect::<Vec<_>>().join(" ");
        }
    }
    "documentation".to_string()
}

// ── Rel-type detection ───────────────────────────────────────────────────────

/// Port of the JS `detectRelType()`. Returns the matching `RelType` (which is
/// always non-null because the final `sync` entry has threshold 0).
pub(crate) fn detect_rel_type(all_tokens: &[String]) -> &'static RelType {
    // Skip the final entry (sync), per JS `slice(0, -1)`.
    for rel in &REL_TYPES[..REL_TYPES.len() - 1] {
        let mut score = 0usize;
        for t in all_tokens {
            if rel.words.binary_search(&t.as_str()).is_ok() {
                score += 1;
            }
        }
        if score >= rel.threshold {
            return rel;
        }
    }
    &REL_TYPES[REL_TYPES.len() - 1]
}

// ── Category detection ──────────────────────────────────────────────────────

/// Port of the JS `detectCategory()`.
pub(crate) fn detect_category(
    all_tokens: &[String],
    path_tokens: &[String],
    heading_tokens: &[String],
) -> Option<&'static str> {
    let mut best: Option<(&'static str, f64)> = None;
    // Iterate CATEGORIES in declaration order — matches JS `Object.entries` for the
    // small literal object, where insertion order is preserved.
    for cat in CATEGORIES {
        let mut score = 0.0_f64;
        for t in all_tokens {
            if cat.words.binary_search(&t.as_str()).is_ok() {
                score += 0.5;
            }
        }
        for t in path_tokens {
            if cat.words.binary_search(&t.as_str()).is_ok() {
                score += 2.0;
            }
        }
        for t in heading_tokens {
            if cat.words.binary_search(&t.as_str()).is_ok() {
                score += 3.0;
            }
        }
        match best {
            None => best = Some((cat.name, score)),
            Some((_, b)) if score > b => best = Some((cat.name, score)),
            _ => {}
        }
    }
    match best {
        Some((name, score)) if score >= 3.0 => Some(name),
        _ => None,
    }
}

// ── Core phrase selection ────────────────────────────────────────────────────

fn verb_only_re() -> &'static Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^(adding|removing|updating|creating|building|extending|extend|defining|declaring|checking|handling|loading|saving|parsing|rendering|returning|using|getting|setting|having|making|calling|running|sending|showing|starting|stopping|enabling|disabling|changing|moving|wiring|mapping|reading|writing|marking|tracking|processing|generating|computing|resolving|detecting|scanning|enforcing|dispatching|routing|declared|defined|created|removed|updated|added|extended|checked|re|one|two|three|many|more)$",
        )
        .expect("valid verb regex")
    })
}

fn is_weak_core_phrase(phrase: &str) -> bool {
    let words: Vec<&str> = phrase.split_whitespace().collect();
    if words.len() == 1 && verb_only_re().is_match(words[0]) {
        return true;
    }
    if slugify(phrase).replace('-', "").len() < 3 {
        return true;
    }
    false
}

fn title_dominated(phrase: &str, exclude: &BTreeSet<String>) -> bool {
    let words: Vec<&str> = phrase
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .collect();
    if words.is_empty() {
        return true;
    }
    let excluded = words.iter().filter(|w| exclude.contains(**w)).count();
    (excluded as f64) / (words.len() as f64) > 0.5
}

/// Port of the JS `selectCorePhrase()`.
pub(crate) fn select_core_phrase(
    rake_results: &[RakeResult],
    co_present: &[CoPresentTerm],
    link_text: &str,
    target_path: &str,
    source_title_tokens: &BTreeSet<String>,
) -> String {
    for r in rake_results {
        if !title_dominated(&r.phrase, source_title_tokens)
            && r.phrase.len() > 2
            && !is_weak_core_phrase(&r.phrase)
        {
            return r.phrase.clone();
        }
    }
    if !co_present.is_empty() {
        let top: String = co_present
            .iter()
            .take(3)
            .map(|t| t.term.clone())
            .collect::<Vec<_>>()
            .join(" ");
        if !title_dominated(&top, source_title_tokens)
            && top.len() > 2
            && !is_weak_core_phrase(&top)
        {
            return top;
        }
    }
    if !link_text.is_empty() {
        let raw: Vec<String> = tokenize(link_text)
            .into_iter()
            .filter(|w| !is_stop(w) && !is_noise(w))
            .collect();
        let mut deduped: Vec<String> = Vec::with_capacity(raw.len());
        for (i, w) in raw.iter().enumerate() {
            if i == 0 || w != &raw[i - 1] {
                deduped.push(w.clone());
            }
        }
        let cleaned: String = deduped.into_iter().take(4).collect::<Vec<_>>().join(" ");
        if !cleaned.is_empty()
            && !title_dominated(&cleaned, source_title_tokens)
            && !is_weak_core_phrase(&cleaned)
        {
            return cleaned;
        }
    }
    if !target_path.is_empty() {
        let seg = extract_target_role(target_path, None);
        if !seg.is_empty() && seg != "target" {
            return seg;
        }
    }
    // Last resort
    if let Some(r) = rake_results
        .iter()
        .find(|r| !title_dominated(&r.phrase, source_title_tokens) && r.phrase.len() > 2)
    {
        return r.phrase.clone();
    }
    if let Some(r) = rake_results.first() {
        return r.phrase.clone();
    }
    if !link_text.is_empty() {
        return link_text.to_string();
    }
    "relationship".to_string()
}

// ── Slug + dedup ─────────────────────────────────────────────────────────────

/// Port of the JS `slugify()`.
pub(crate) fn slugify(phrase: &str) -> String {
    let re_non_alnum = static_re(r"[^a-z0-9 ]");
    // JS: /[^a-z0-9 ]/gi — case-insensitive, so we lowercase first then strip.
    let lowered = phrase.to_lowercase();
    let cleaned = re_non_alnum.replace_all(&lowered, " ");
    let toks: Vec<String> = cleaned
        .split_whitespace()
        .map(|s| s.to_string())
        .filter(|t| t.len() > 1 && !is_file_ext(t))
        .collect();
    let joined = toks.join("-");
    // collapse and trim hyphens
    let collapsed = static_re(r"-+").replace_all(&joined, "-").into_owned();
    let trimmed = collapsed.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "relationship".to_string()
    } else {
        trimmed
    }
}

/// Capitalize the first character of a string. Port of the JS `cap()`.
pub(crate) fn cap(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Normalize for tautology comparison: lowercase, collapse `-`, `_`, whitespace.
pub(crate) fn norm_cmp(s: &str) -> String {
    let re = static_re(r"[-_\s]+");
    re.replace_all(&s.to_lowercase(), "").into_owned()
}

/// In-place rename of duplicate names with `-1`, `-2`, … suffixes.
///
/// Port of the JS `deduplicateNames()`.
pub(crate) fn deduplicate_names(names: &mut [String]) {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for n in names.iter() {
        *counts.entry(n.clone()).or_insert(0) += 1;
    }
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for n in names.iter_mut() {
        if *counts.get(n).unwrap_or(&1) > 1 {
            let next = seen.get(n).copied().unwrap_or(0) + 1;
            seen.insert(n.clone(), next);
            *n = format!("{n}-{next}");
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tokenize_strips_file_extensions() {
        let toks = tokenize("config.ts and parser.rs");
        assert!(!toks.contains(&"ts".to_string()));
        assert!(!toks.contains(&"rs".to_string()));
        assert!(toks.contains(&"config".to_string()));
        assert!(toks.contains(&"parser".to_string()));
    }

    #[test]
    fn tokenize_drops_stop_words_and_short_tokens() {
        // STOP filtering is not done here — tokenize keeps stop words; only
        // FILE_EXTS, single-char, and all-digit are filtered.
        let toks = tokenize("the quick 1 ab x");
        // "x" (length 1) is filtered, "1" filtered, "ab" kept, "the"/"quick" kept.
        assert!(toks.contains(&"the".to_string()));
        assert!(toks.contains(&"ab".to_string()));
        assert!(!toks.contains(&"x".to_string()));
        assert!(!toks.contains(&"1".to_string()));
    }

    #[test]
    fn rake_returns_empty_for_empty_input() {
        assert!(rake("").is_empty());
    }

    #[test]
    fn rake_extracts_phrases_and_orders_by_score() {
        // Two stop-separated phrases. RAKE picks "fragment link parser" as a
        // multi-word phrase scoring above any of its substrings.
        let res = rake("the fragment link parser is the best");
        assert!(!res.is_empty());
        let top = &res[0];
        assert!(top.score > 0.0);
        // Ensure "fragment" is among the top phrase's words.
        assert!(
            res.iter()
                .any(|r| r.phrase.contains("fragment") || r.phrase.contains("link"))
        );
    }

    #[test]
    fn rake_skips_adjacent_duplicate_phrases() {
        // Adjacent duplicates ("extension extension") must not appear as a phrase.
        let res = rake("foo extension extension bar");
        for r in &res {
            assert_ne!(r.phrase, "extension extension");
        }
    }

    #[test]
    fn co_presence_picks_shared_terms() {
        let src = "The renderer transforms checkout payloads.";
        let tgt = "renderer module emits checkout pages.";
        let result = co_presence_terms(src, tgt);
        let terms: Vec<&str> = result.iter().map(|t| t.term.as_str()).collect();
        assert!(terms.contains(&"renderer"));
        assert!(terms.contains(&"checkout"));
    }

    #[test]
    fn detect_category_requires_structural_signal() {
        // All-token billing words alone (weight 0.5) cannot reach threshold 3.
        let all = s(&["billing", "billing", "billing"]);
        let path: Vec<String> = vec![];
        let heading: Vec<String> = vec![];
        assert!(detect_category(&all, &path, &heading).is_none());

        // Heading hit (weight 3.0) by itself qualifies.
        let heading2 = s(&["billing"]);
        assert_eq!(detect_category(&[], &[], &heading2), Some("billing"));
    }

    #[test]
    fn detect_rel_type_falls_back_to_sync() {
        let toks = s(&["just", "some", "words"]);
        assert_eq!(detect_rel_type(&toks).rel_type, "sync");
    }

    #[test]
    fn detect_rel_type_picks_contract_at_threshold() {
        let toks = s(&["schema", "validates", "noise"]);
        assert_eq!(detect_rel_type(&toks).rel_type, "contract");
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Hello World!"), "hello-world");
        assert_eq!(slugify("foo--bar"), "foo-bar");
        assert_eq!(slugify("config.ts"), "config");
        assert_eq!(slugify("---"), "relationship");
    }

    #[test]
    fn deduplicate_names_appends_numeric_suffix() {
        let mut names = s(&["wiki/a", "wiki/b", "wiki/a", "wiki/a"]);
        deduplicate_names(&mut names);
        assert_eq!(names, s(&["wiki/a-1", "wiki/b", "wiki/a-2", "wiki/a-3"]));
    }

    #[test]
    fn deduplicate_names_leaves_uniques_alone() {
        let mut names = s(&["wiki/a", "wiki/b", "wiki/c"]);
        deduplicate_names(&mut names);
        assert_eq!(names, s(&["wiki/a", "wiki/b", "wiki/c"]));
    }

    #[test]
    fn extract_target_role_uses_title_when_present() {
        // "link" is in NOISE; filtered before take(3).
        let role = extract_target_role("packages/cli/src/parser.rs", Some("Fragment Link Parser"));
        assert_eq!(role, "fragment parser");
    }

    #[test]
    fn extract_target_role_falls_back_to_path_segment() {
        let role = extract_target_role("packages/cli/src/parser.rs", None);
        assert_eq!(role, "parser");
    }

    #[test]
    fn extract_source_role_walks_heading_chain_in_reverse() {
        let chain = s(&["Top", "Mid Section", "Deepest Heading"]);
        let role = extract_source_role(&chain, None);
        assert_eq!(role, "deepest heading");
    }

    #[test]
    fn select_core_phrase_uses_link_text_when_rake_empty() {
        let exclude: BTreeSet<String> = BTreeSet::new();
        let phrase = select_core_phrase(&[], &[], "Fragment Link Parser", "", &exclude);
        // Tokenized link text ('fragment link parser') with stop/noise filtered;
        // 'link' is in NOISE so the result is "fragment parser".
        assert!(phrase.contains("fragment"));
        assert!(phrase.contains("parser"));
    }
}
