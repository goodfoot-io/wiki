use crate::git::{
    apply_ref_transaction, git_stdout, git_stdout_with_identity, resolve_ref_oid_optional,
    RefUpdate,
};
use crate::mesh::commit::run_test_hook;
use crate::validation::validate_mesh_name;
use anyhow::{Result, anyhow};

pub fn remove_mesh(repo: &gix::Repository, name: &str) -> Result<()> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let mesh_ref = format!("refs/meshes/v1/{name}");
    let current_tip = resolve_ref_oid_optional(work_dir, &mesh_ref)?
        .ok_or_else(|| anyhow!("mesh `{name}` does not exist"))?;
    run_test_hook(work_dir, "remove_mesh_before_transaction");
    apply_ref_transaction(
        work_dir,
        &[RefUpdate::Delete {
            name: mesh_ref,
            expected_old_oid: current_tip,
        }],
    )?;
    Ok(())
}

pub fn rename_mesh(
    repo: &gix::Repository,
    old_name: &str,
    new_name: &str,
    keep: bool,
) -> Result<()> {
    validate_mesh_name(new_name)?;
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let old_ref = format!("refs/meshes/v1/{old_name}");
    let new_ref = format!("refs/meshes/v1/{new_name}");
    let commit_oid = git_stdout(work_dir, ["rev-parse", &old_ref])?;
    let mut updates = vec![RefUpdate::Create {
        name: new_ref,
        new_oid: commit_oid.clone(),
    }];
    if !keep {
        updates.push(RefUpdate::Delete {
            name: old_ref,
            expected_old_oid: commit_oid,
        });
    }
    apply_ref_transaction(work_dir, &updates)?;
    Ok(())
}

pub fn restore_mesh(repo: &gix::Repository, name: &str, commit_ish: &str) -> Result<()> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow!("Bare repositories are not supported"))?;
    let mesh_ref = format!("refs/meshes/v1/{name}");
    let revision = if commit_ish == "HEAD" {
        mesh_ref.clone()
    } else if let Some(suffix) = commit_ish.strip_prefix("HEAD") {
        format!("{mesh_ref}{suffix}")
    } else {
        commit_ish.to_string()
    };
    let commit_oid = git_stdout(work_dir, ["rev-parse", &revision])?;
    let current_tip = resolve_ref_oid_optional(work_dir, &mesh_ref)?;
    let tree_oid = git_stdout(work_dir, ["show", "-s", "--format=%T", &commit_oid])?;
    let message = git_stdout(work_dir, ["show", "-s", "--format=%B", &commit_oid])?;

    let mut args = vec![
        "commit-tree".to_string(),
        tree_oid,
        "-m".to_string(),
        message,
    ];
    if let Some(parent) = current_tip.as_deref() {
        args.push("-p".to_string());
        args.push(parent.to_string());
    }

    let restored_commit = git_stdout_with_identity(work_dir, args.iter().map(String::as_str))?;
    run_test_hook(work_dir, "restore_mesh_before_transaction");
    apply_ref_transaction(
        work_dir,
        &[match current_tip {
            Some(parent) => RefUpdate::Update {
                name: mesh_ref,
                new_oid: restored_commit,
                expected_old_oid: parent,
            },
            None => RefUpdate::Create {
                name: mesh_ref,
                new_oid: restored_commit,
            },
        }],
    )?;
    Ok(())
}
