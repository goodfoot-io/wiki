use anyhow::{Result, anyhow};
use std::path::Path;
use std::process::Command;

pub(crate) enum RefUpdate {
    Create {
        name: String,
        new_oid: String,
    },
    Update {
        name: String,
        new_oid: String,
        expected_old_oid: String,
    },
    Delete {
        name: String,
        expected_old_oid: String,
    },
}

pub(crate) fn apply_ref_transaction(work_dir: &Path, updates: &[RefUpdate]) -> Result<()> {
    let mut input = String::from("start\n");
    for update in updates {
        match update {
            RefUpdate::Create { name, new_oid } => {
                input.push_str(&format!("create {name} {new_oid}\n"));
            }
            RefUpdate::Update {
                name,
                new_oid,
                expected_old_oid,
            } => {
                input.push_str(&format!("update {name} {new_oid} {expected_old_oid}\n"));
            }
            RefUpdate::Delete {
                name,
                expected_old_oid,
            } => {
                input.push_str(&format!("delete {name} {expected_old_oid}\n"));
            }
        }
    }
    input.push_str("prepare\ncommit\n");
    git_with_input(work_dir, ["update-ref", "--stdin"], &input)?;
    Ok(())
}

pub(crate) fn is_reference_transaction_conflict(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    message.contains("cannot lock ref")
        || message.contains("reference already exists")
        || message.contains("is at ")
        || message.contains("expected ")
}

pub fn read_git_text(repo: &gix::Repository, object: &str) -> Result<String> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    git_stdout(work_dir, ["cat-file", "-p", object])
}

pub(crate) fn resolve_ref_oid_optional(work_dir: &Path, ref_name: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .current_dir(work_dir)
        .args(["rev-parse", "--verify", "--quiet", ref_name])
        .output()?;
    match output.status.code() {
        Some(0) => Ok(Some(String::from_utf8(output.stdout)?.trim().to_string())),
        Some(1) => Ok(None),
        _ => anyhow::bail!(
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

pub(crate) fn git_show_file_lines(
    work_dir: &Path,
    commit_oid: &str,
    path: &str,
) -> Result<Vec<String>> {
    let output = git_stdout(work_dir, ["show", &format!("{commit_oid}:{path}")])?;
    Ok(output
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

pub(crate) fn git_stdout<I, S>(work_dir: &std::path::Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let output = Command::new("git")
        .current_dir(work_dir)
        .args(args.into_iter().map(|arg| arg.as_ref().to_string()))
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Like [`git_stdout`], but returns the process stdout without trimming.
/// Use this when the exact byte layout matters — e.g. when feeding a stored
/// object to a parser that enforces §4.1 trailing-newline invariants.
pub(crate) fn git_stdout_raw<I, S>(work_dir: &std::path::Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let output = Command::new("git")
        .current_dir(work_dir)
        .args(args.into_iter().map(|arg| arg.as_ref().to_string()))
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

pub(crate) fn git_stdout_optional<I, S>(work_dir: &Path, args: I) -> Result<Option<String>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let output = Command::new("git")
        .current_dir(work_dir)
        .args(args.into_iter().map(|arg| arg.as_ref().to_string()))
        .output()?;
    match output.status.code() {
        Some(0) => Ok(Some(String::from_utf8(output.stdout)?.trim().to_string())),
        Some(1) => Ok(None),
        _ => anyhow::bail!(
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

pub(crate) fn git_stdout_lines<I, S>(work_dir: &Path, args: I) -> Result<Vec<String>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    Ok(git_stdout_optional(work_dir, args)?
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

pub(crate) fn git_stdout_with_identity<I, S>(work_dir: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let output = Command::new("git")
        .current_dir(work_dir)
        .env("GIT_AUTHOR_NAME", "git-mesh")
        .env("GIT_AUTHOR_EMAIL", "git-mesh@example.com")
        .env("GIT_COMMITTER_NAME", "git-mesh")
        .env("GIT_COMMITTER_EMAIL", "git-mesh@example.com")
        .args(args.into_iter().map(|arg| arg.as_ref().to_string()))
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

pub(crate) fn git_with_input<I, S>(
    work_dir: &std::path::Path,
    args: I,
    input: &str,
) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    use std::io::Write;

    let mut child = Command::new("git")
        .current_dir(work_dir)
        .args(args.into_iter().map(|arg| arg.as_ref().to_string()))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("missing stdin"))?;
        stdin.write_all(input.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    anyhow::ensure!(
        output.status.success(),
        "git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}
