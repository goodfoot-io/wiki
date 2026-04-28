use std::path::Path;
use std::sync::OnceLock;

use pulldown_cmark::{Options, Parser, html};
use regex::Regex;
use syntect::highlighting::ThemeSet;
use syntect::html::{ClassStyle, ClassedHTMLGenerator, css_for_theme_with_class_style};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::headings::github_slug;
use crate::index::WikiIndex;
use crate::parser::{LinkKind, parse_fragment_links, scrub_non_content};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderMode {
    Fragment { file_base_url: Option<String> },
    FullPage,
}

struct SyntectAssets {
    syntax_set: SyntaxSet,
    css: String,
}

struct Replacement {
    start: usize,
    end: usize,
    text: String,
}

const PAGE_CSS: &str = r#"
:root {
  color-scheme: light;
  --bg: #f6f8fa;
  --panel: #ffffff;
  --border: #d0d7de;
  --text: #1f2328;
  --muted: #59636e;
  --link: #0969da;
  --code-bg: #f6f8fa;
}

* { box-sizing: border-box; }

body {
  margin: 0;
  padding: 0;
  font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  color: var(--text);
  background: linear-gradient(180deg, #fbfcfe 0%, var(--bg) 100%);
}

a { color: var(--link); }

header {
  position: sticky;
  top: 0;
  z-index: 10;
  display: flex;
  gap: 1rem;
  align-items: center;
  justify-content: space-between;
  padding: 1rem 1.5rem;
  border-bottom: 1px solid var(--border);
  background: rgba(255, 255, 255, 0.92);
  backdrop-filter: blur(8px);
}

header .brand {
  font-weight: 700;
  letter-spacing: 0.02em;
  text-decoration: none;
  color: var(--text);
}

header nav {
  display: flex;
  gap: 1rem;
  align-items: center;
  flex-wrap: wrap;
}

header form {
  display: flex;
  gap: 0.5rem;
}

input[type="search"] {
  min-width: 16rem;
  padding: 0.6rem 0.8rem;
  border: 1px solid var(--border);
  border-radius: 0.6rem;
  background: var(--panel);
}

button {
  padding: 0.6rem 0.9rem;
  border: 1px solid var(--border);
  border-radius: 0.6rem;
  background: var(--panel);
  color: var(--text);
  cursor: pointer;
}

main {
  width: min(980px, calc(100vw - 2rem));
  margin: 2rem auto 4rem;
}

article,
.panel {
  padding: 2rem;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 1rem;
  box-shadow: 0 10px 30px rgba(31, 35, 40, 0.06);
}

article > :first-child,
.panel > :first-child {
  margin-top: 0;
}

article img {
  max-width: 100%;
}

pre {
  overflow-x: auto;
  padding: 1rem;
  border-radius: 0.75rem;
  background: var(--code-bg);
}

code {
  font-family: "SFMono-Regular", SFMono-Regular, ui-monospace, Menlo, monospace;
}

pre code {
  background: transparent;
  padding: 0;
}

:not(pre) > code {
  padding: 0.15rem 0.35rem;
  border-radius: 0.4rem;
  background: var(--code-bg);
}

blockquote {
  margin-left: 0;
  padding-left: 1rem;
  border-left: 4px solid var(--border);
  color: var(--muted);
}

ul.page-list {
  padding-left: 1.2rem;
}

.search-meta {
  margin-bottom: 1rem;
  color: var(--muted);
}
"#;

const LIVE_RELOAD_SCRIPT: &str = r#"<script>
const source = new EventSource('/_sse');
source.onmessage = () => location.reload();
</script>"#;

pub fn render_html(content: &str, mode: RenderMode, index: &WikiIndex) -> String {
    let markdown = preprocess_markdown(content, &mode, index);
    let parser = Parser::new_ext(&markdown, Options::all());
    let mut rendered = String::new();
    html::push_html(&mut rendered, parser);

    match mode {
        RenderMode::Fragment { .. } => rendered,
        RenderMode::FullPage => highlight_code_blocks(&rendered),
    }
}

pub fn wrap_page(title: &str, body: &str, live_reload: bool) -> String {
    let reload_script = if live_reload { LIVE_RELOAD_SCRIPT } else { "" };
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{}</title><style>{}</style><style>{}</style></head><body><header><a class=\"brand\" href=\"/\">Wiki</a><nav><form action=\"/search\" method=\"get\"><input type=\"search\" name=\"q\" placeholder=\"Search wiki\" /><button type=\"submit\">Search</button></form><a href=\"/\">All pages</a></nav></header><main><article>{}</article></main>{}</body></html>",
        escape_html_text(title),
        PAGE_CSS,
        syntect_assets().css,
        body,
        reload_script
    )
}

fn preprocess_markdown(content: &str, mode: &RenderMode, index: &WikiIndex) -> String {
    let scrubbed = scrub_non_content(content);
    let mut replacements = collect_wikilink_replacements(content, &scrubbed, index);
    replacements.extend(collect_fragment_replacements(
        content, &scrubbed, mode, index,
    ));

    if replacements.is_empty() {
        return content.to_string();
    }

    replacements.sort_by_key(|replacement| replacement.start);

    let mut output = String::with_capacity(content.len());
    let mut cursor = 0usize;
    for replacement in replacements {
        if replacement.start < cursor {
            continue;
        }
        output.push_str(&content[cursor..replacement.start]);
        output.push_str(&replacement.text);
        cursor = replacement.end;
    }
    output.push_str(&content[cursor..]);
    output
}

fn collect_wikilink_replacements(
    content: &str,
    scrubbed: &str,
    index: &WikiIndex,
) -> Vec<Replacement> {
    wikilink_re()
        .find_iter(scrubbed)
        .filter_map(|matched| {
            let raw_inner = &content[matched.start() + 2..matched.end() - 2];
            let (target_part, display) = raw_inner
                .split_once('|')
                .map_or((raw_inner, None), |(target, display)| {
                    (target, Some(display))
                });
            let (title, heading) = target_part
                .split_once('#')
                .map_or((target_part, None), |(title, heading)| {
                    (title, Some(heading))
                });

            let title = title.trim();
            if title.is_empty() {
                return None;
            }

            let page = index.resolve_page(title).ok()??;
            let label = display.unwrap_or(&page.title);
            let mut href = format!("/{}", encode_path_segment(&page.title));
            if let Some(heading) = heading.filter(|heading| !heading.trim().is_empty()) {
                href.push('#');
                href.push_str(&github_slug(heading.trim()));
            }

            Some(Replacement {
                start: matched.start(),
                end: matched.end(),
                text: format!("[{}]({href})", escape_markdown_label(label)),
            })
        })
        .collect()
}

fn collect_fragment_replacements(
    content: &str,
    scrubbed: &str,
    mode: &RenderMode,
    index: &WikiIndex,
) -> Vec<Replacement> {
    markdown_link_re()
        .find_iter(scrubbed)
        .filter_map(|matched| {
            let original = &content[matched.start()..matched.end()];
            let link = parse_fragment_links(original).into_iter().next()?;
            if link.kind == LinkKind::External {
                return None;
            }
            let replacement = match mode {
                RenderMode::FullPage => render_fragment_as_code_block(&link, index),
                RenderMode::Fragment { file_base_url } => {
                    render_fragment_as_link(&link, file_base_url.as_deref(), index.repo_root())
                }
            }?;

            Some(Replacement {
                start: matched.start(),
                end: matched.end(),
                text: replacement,
            })
        })
        .collect()
}

fn render_fragment_as_code_block(
    link: &crate::parser::FragmentLink,
    index: &WikiIndex,
) -> Option<String> {
    let repo_path = Path::new(&link.path);
    let file_path = index.repo_root().join(repo_path);
    let file_content = std::fs::read_to_string(&file_path).ok()?;
    let snippet = slice_lines(&file_content, link.start_line, link.end_line)?;
    let language = infer_language(repo_path);
    Some(format!("```{language}\n{snippet}\n```"))
}

fn render_fragment_as_link(
    link: &crate::parser::FragmentLink,
    file_base_url: Option<&str>,
    repo_root: &Path,
) -> Option<String> {
    let path = if let Some(base_url) = file_base_url {
        let base = base_url.trim_end_matches('/');
        format!("{base}/{}", encode_path(Path::new(&link.path)))
    } else {
        let absolute = repo_root.join(&link.path);
        format!("file://{}", encode_file_url_path(&absolute))
    };

    let mut href = path;
    if let Some(fragment) = line_fragment(link.start_line, link.end_line) {
        href.push('#');
        href.push_str(&fragment);
    }

    Some(format!(
        "[{}]({href})",
        escape_markdown_label(&link.original_text)
    ))
}

fn slice_lines(content: &str, start: Option<u32>, end: Option<u32>) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Some(String::new());
    }

    let start_idx = start.unwrap_or(1).max(1) as usize;
    let end_idx = end
        .unwrap_or(start.unwrap_or(lines.len() as u32))
        .max(start.unwrap_or(1)) as usize;
    if start_idx > lines.len() || end_idx > lines.len() || start_idx > end_idx {
        return None;
    }

    Some(lines[start_idx - 1..end_idx].join("\n"))
}

fn infer_language(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_string()
}

fn highlight_code_blocks(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut cursor = 0usize;

    for captures in code_block_re().captures_iter(html) {
        let matched = captures.get(0).expect("match");
        output.push_str(&html[cursor..matched.start()]);

        let language = captures
            .get(1)
            .map(|capture| capture.as_str())
            .unwrap_or_default();
        let code = captures
            .get(2)
            .map(|capture| decode_html_entities(capture.as_str()))
            .unwrap_or_default();
        output.push_str(&highlight_snippet(&code, language));

        cursor = matched.end();
    }

    output.push_str(&html[cursor..]);
    output
}

fn highlight_snippet(code: &str, language: &str) -> String {
    let assets = syntect_assets();
    let syntax = assets
        .syntax_set
        .find_syntax_by_token(language)
        .or_else(|| assets.syntax_set.find_syntax_by_extension(language))
        .unwrap_or_else(|| assets.syntax_set.find_syntax_plain_text());

    let mut generator =
        ClassedHTMLGenerator::new_with_class_style(syntax, &assets.syntax_set, ClassStyle::Spaced);
    for line in LinesWithEndings::from(code) {
        if generator
            .parse_html_for_line_which_includes_newline(line)
            .is_err()
        {
            return format!(
                "<pre><code class=\"language-{}\">{}</code></pre>",
                escape_html_attr(language),
                escape_html_text(code)
            );
        }
    }
    generator.finalize()
}

fn syntect_assets() -> &'static SyntectAssets {
    static ASSETS: OnceLock<SyntectAssets> = OnceLock::new();
    ASSETS.get_or_init(|| {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme_set = ThemeSet::load_defaults();
        let theme = theme_set
            .themes
            .get("InspiredGitHub")
            .cloned()
            .or_else(|| theme_set.themes.values().next().cloned())
            .expect("syntect ships at least one theme");
        let css = css_for_theme_with_class_style(&theme, ClassStyle::Spaced).unwrap_or_default();

        SyntectAssets { syntax_set, css }
    })
}

fn wikilink_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[\[([^\[\]]+)\]\]").expect("valid wikilink regex"))
}

fn markdown_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[[^\[\]]*\]\(([^)]*)\)").expect("valid markdown link regex"))
}

fn code_block_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?s)<pre><code class="language-([^"]+)">(.*?)</code></pre>"#)
            .expect("valid code block regex")
    })
}

fn line_fragment(start: Option<u32>, end: Option<u32>) -> Option<String> {
    match (start, end) {
        (Some(start), Some(end)) => Some(format!("L{start}-L{end}")),
        (Some(start), None) => Some(format!("L{start}")),
        _ => None,
    }
}

fn encode_file_url_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let prefixed = if raw.starts_with('/') {
        raw.into_owned()
    } else {
        format!("/{raw}")
    };
    encode_bytes(&prefixed, true)
}

fn encode_path(path: &Path) -> String {
    encode_bytes(&path.to_string_lossy(), true)
}

fn encode_path_segment(segment: &str) -> String {
    encode_bytes(segment, false)
}

fn encode_bytes(value: &str, keep_slashes: bool) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        let safe = matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~')
            || (keep_slashes && byte == b'/');
        if safe {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn escape_markdown_label(label: &str) -> String {
    label.replace('[', r"\[").replace(']', r"\]")
}

fn decode_html_entities(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn escape_html_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_html_attr(value: &str) -> String {
    escape_html_text(value).replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use tempfile::TempDir;

    use super::*;

    struct TestRepo {
        dir: TempDir,
    }

    impl TestRepo {
        fn new() -> Self {
            let dir = TempDir::new().expect("tempdir");
            let repo = Self { dir };
            repo.git(&["init"]);
            repo.git(&["checkout", "-b", "main"]);
            repo
        }

        fn path(&self) -> &Path {
            self.dir.path()
        }

        fn create_file(&self, path: &str, content: &str) {
            let full = self.dir.path().join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("create dirs");
            }
            fs::write(full, content).expect("write file");
        }

        fn commit_all(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-m", message]);
        }

        fn git(&self, args: &[&str]) {
            let output = Command::new("git")
                .current_dir(self.dir.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "Test Author")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test Committer")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .output()
                .expect("spawn git");
            assert!(
                output.status.success(),
                "git {:?} failed:\n{}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn render_html_resolves_wikilinks() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/target.md",
            "---\ntitle: Target Page\nsummary: Summary.\n---\n# Heading\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let rendered = render_html(
            "See [[Target Page|this page]].",
            RenderMode::Fragment {
                file_base_url: None,
            },
            &index,
        );

        assert!(rendered.contains(r#"<a href="/Target%20Page">this page</a>"#));
    }

    #[test]
    fn render_html_substitutes_fragment_links_in_full_page_mode() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/example.md",
            "---\ntitle: Example\nsummary: Summary.\n---\nBody.\n",
        );
        repo.create_file("src/lib.rs", "fn alpha() {}\nfn beta() {}\n");
        repo.commit_all("initial");

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let rendered = render_html("[fragment](src/lib.rs#L1-L2)", RenderMode::FullPage, &index);

        assert!(rendered.contains("alpha"));
        assert!(!rendered.contains("src/lib.rs#L"));
    }

    #[test]
    fn render_html_substitutes_fragment_links_in_fragment_mode() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/example.md",
            "---\ntitle: Example\nsummary: Summary.\n---\nBody.\n",
        );
        repo.create_file("src/lib.rs", "fn alpha() {}\n");
        repo.commit_all("initial");

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let rendered = render_html(
            "[fragment](src/lib.rs#L1-L1)",
            RenderMode::Fragment {
                file_base_url: Some("https://example.test/base".to_string()),
            },
            &index,
        );

        assert!(rendered.contains(r#"href="https://example.test/base/src/lib.rs#L1-L1""#));
    }

    #[test]
    fn render_html_leaves_fenced_code_block_contents_untouched() {
        let repo = TestRepo::new();
        let _wiki_dir = crate::test_support::set_wiki_dir("wiki");
        repo.create_file(
            "wiki/example.md",
            "---\ntitle: Example\nsummary: Summary.\n---\nBody.\n",
        );

        let index = WikiIndex::prepare(repo.path()).expect("prepare");
        let rendered = render_html(
            "```md\n[[Example]]\n```",
            RenderMode::Fragment {
                file_base_url: None,
            },
            &index,
        );

        assert!(rendered.contains("[[Example]]"));
        assert!(!rendered.contains(r#"<a href="/Example">"#));
    }
}
