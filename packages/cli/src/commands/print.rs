use std::path::Path;

use miette::Result;

use crate::index::WikiIndex;

use super::summary::render_not_found;

pub fn run(title: &str, json: bool, repo_root: &Path) -> Result<i32> {
    let index = WikiIndex::prepare(repo_root)?;
    match index.resolve_page(title)? {
        Some(page) => {
            if json {
                let output = serde_json::json!({
                    "title": page.title,
                    "file": page.file,
                    "content": page.content,
                });
                println!("{}", serde_json::to_string_pretty(&output).unwrap());
            } else {
                print!("{}", page.content);
            }
            Ok(0)
        }
        None => {
            let suggestions = index.suggest(title)?;
            if json {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "error": format!("page '{}' not found", title),
                        "suggestions": suggestions,
                    })
                );
            } else {
                eprintln!("{}", render_not_found(title, &suggestions, repo_root));
            }
            Ok(1)
        }
    }
}
