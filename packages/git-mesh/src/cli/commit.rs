use crate::cli::{parse_copy_detection, parse_link_pair, parse_range_pair};
use anyhow::{Context, Result, anyhow};
use git_mesh::{commit_mesh, read_mesh_at, validate_mesh_name, CommitInput};
use std::fs;
use std::process::Command as ProcessCommand;

pub(crate) fn run_commit(repo: &gix::Repository, sub_matches: &clap::ArgMatches) -> Result<()> {
    let name = sub_matches.get_one::<String>("name").unwrap();
    validate_mesh_name(name)?;

    let default_copy_detection = sub_matches
        .get_one::<String>("copy-detection")
        .map(|value| parse_copy_detection(value))
        .transpose()?;
    let default_ignore_whitespace = if sub_matches.get_flag("no-ignore-whitespace") {
        Some(false)
    } else {
        Some(true)
    };

    let adds = sub_matches
        .get_many::<String>("link")
        .into_iter()
        .flatten()
        .map(|value| parse_link_pair(value, default_copy_detection, default_ignore_whitespace))
        .collect::<Result<Vec<_>>>()?;
    let removes = sub_matches
        .get_many::<String>("unlink")
        .into_iter()
        .flatten()
        .map(|value| parse_range_pair(value))
        .collect::<Result<Vec<_>>>()?;

    let message = resolve_commit_message(repo, name, sub_matches)?;
    let anchor_sha = sub_matches.get_one::<String>("anchor").cloned();
    let amend = sub_matches.get_flag("amend");

    commit_mesh(
        repo,
        CommitInput {
            name: name.clone(),
            adds,
            removes,
            message,
            anchor_sha,
            expected_tip: None,
            amend,
        },
    )?;

    let ref_name = format!("refs/meshes/v1/{name}");
    println!("updated {ref_name}");
    Ok(())
}

fn resolve_commit_message(
    repo: &gix::Repository,
    name: &str,
    matches: &clap::ArgMatches,
) -> Result<String> {
    if let Some(message) = matches.get_one::<String>("message") {
        return Ok(message.clone());
    }

    if let Some(path) = matches.get_one::<String>("message-file") {
        return fs::read_to_string(path)
            .with_context(|| format!("failed to read message file `{path}`"));
    }

    if matches.get_flag("edit") {
        let template = if matches.get_flag("amend") {
            read_mesh_at(repo, name, None)
                .map(|mesh| mesh.message)
                .unwrap_or_default()
        } else {
            String::new()
        };
        return edit_commit_message(repo, &template);
    }

    Err(anyhow!("missing commit message"))
}

fn edit_commit_message(repo: &gix::Repository, initial: &str) -> Result<String> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let path = std::env::temp_dir().join(format!("git-mesh-msg-{}.txt", uuid::Uuid::new_v4()));
    fs::write(&path, initial)?;

    let editor = std::env::var("GIT_EDITOR")
        .or_else(|_| std::env::var("EDITOR"))
        .or_else(|_| git_editor(work_dir))
        .unwrap_or_else(|_| "vi".to_string());

    let status = ProcessCommand::new("sh")
        .current_dir(work_dir)
        .arg("-lc")
        .arg("\"$1\" \"$2\"")
        .arg("git-mesh-editor")
        .arg(editor)
        .arg(
            path.to_str()
                .ok_or_else(|| anyhow!("message file path is not valid UTF-8"))?,
        )
        .status()?;
    anyhow::ensure!(status.success(), "editor exited with status {status}");

    let message = fs::read_to_string(&path)?;
    let _ = fs::remove_file(&path);
    anyhow::ensure!(
        !message.trim().is_empty(),
        "aborting commit due to empty commit message"
    );
    Ok(message)
}

fn git_editor(work_dir: &std::path::Path) -> Result<String> {
    let output = ProcessCommand::new("git")
        .current_dir(work_dir)
        .args(["var", "GIT_EDITOR"])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "git var GIT_EDITOR failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}
