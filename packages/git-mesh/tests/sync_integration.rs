mod support;

use anyhow::Result;
use support::{BareRepo, TestRepo};

fn mesh_fetch_refspecs(repo: &TestRepo, remote: &str) -> Result<Vec<String>> {
    repo.config_get_all(&format!("remote.{remote}.fetch"))
}

fn mesh_push_refspecs(repo: &TestRepo, remote: &str) -> Result<Vec<String>> {
    repo.config_get_all(&format!("remote.{remote}.push"))
}

fn mesh_only_refspecs(refspecs: &[String]) -> Vec<String> {
    refspecs
        .iter()
        .filter(|refspec| {
            refspec.starts_with("+refs/links/") || refspec.starts_with("+refs/meshes/")
        })
        .cloned()
        .collect()
}

#[test]
fn push_and_fetch_sync_mesh_refs_across_repositories() -> Result<()> {
    let remote = BareRepo::new()?;
    let source = TestRepo::new()?;
    source.run_git(["branch", "-M", "main"])?;
    source.add_remote("origin", remote.path())?;
    source.run_git(["push", "origin", "HEAD:refs/heads/main"])?;
    remote.set_head("main")?;

    let clone = TestRepo::clone_from(remote.path())?;

    source.mesh_stdout([
        "commit",
        "shared-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Shared mesh",
    ])?;
    let source_mesh_tip = source.read_ref("refs/meshes/v1/shared-mesh")?;
    let source_link_id = source.show_file(&source_mesh_tip, "links")?;
    let source_link_tip = source.read_ref(&format!("refs/links/v1/{source_link_id}"))?;

    source.mesh_stdout(["push"])?;
    clone.mesh_stdout(["fetch"])?;

    assert_eq!(
        clone.read_ref("refs/meshes/v1/shared-mesh")?,
        source_mesh_tip
    );
    assert_eq!(
        clone.read_ref(&format!("refs/links/v1/{source_link_id}"))?,
        source_link_tip
    );

    Ok(())
}

#[test]
fn sync_bootstrap_is_idempotent() -> Result<()> {
    let remote = BareRepo::new()?;
    let repo = TestRepo::new()?;
    repo.run_git(["branch", "-M", "main"])?;
    repo.add_remote("origin", remote.path())?;
    repo.run_git(["push", "origin", "HEAD:refs/heads/main"])?;
    remote.set_head("main")?;

    repo.mesh_stdout(["fetch"])?;
    let first_fetch = mesh_fetch_refspecs(&repo, "origin")?;
    let first_push = mesh_push_refspecs(&repo, "origin")?;

    repo.mesh_stdout(["push"])?;
    repo.mesh_stdout(["fetch"])?;

    assert_eq!(mesh_fetch_refspecs(&repo, "origin")?, first_fetch);
    assert_eq!(mesh_push_refspecs(&repo, "origin")?, first_push);
    assert_eq!(
        mesh_only_refspecs(&first_fetch),
        vec![
            "+refs/links/*:refs/links/*".to_string(),
            "+refs/meshes/*:refs/meshes/*".to_string(),
        ]
    );
    assert_eq!(
        mesh_only_refspecs(&first_push),
        mesh_only_refspecs(&first_fetch)
    );

    Ok(())
}

#[test]
fn sync_respects_explicit_remote_override_and_default_remote_config() -> Result<()> {
    let origin = BareRepo::new()?;
    let upstream = BareRepo::new()?;
    let repo = TestRepo::new()?;
    repo.add_remote("origin", origin.path())?;
    repo.add_remote("upstream", upstream.path())?;
    repo.set_config("mesh.defaultRemote", "upstream")?;

    repo.mesh_stdout([
        "commit",
        "override-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Override mesh",
    ])?;

    repo.mesh_stdout(["push", "origin"])?;
    let mesh_tip = repo.read_ref("refs/meshes/v1/override-mesh")?;
    assert_eq!(
        origin.git_output(["rev-parse", "refs/meshes/v1/override-mesh"])?,
        mesh_tip
    );
    assert!(
        upstream
            .run_git(["rev-parse", "refs/meshes/v1/override-mesh"])
            .is_err()
    );

    repo.mesh_stdout(["push"])?;
    assert_eq!(
        upstream.git_output(["rev-parse", "refs/meshes/v1/override-mesh"])?,
        mesh_tip
    );

    Ok(())
}

#[test]
fn fetched_mesh_refs_continue_to_propagate_between_repositories() -> Result<()> {
    let remote = BareRepo::new()?;
    let source = TestRepo::new()?;
    source.run_git(["branch", "-M", "main"])?;
    source.add_remote("origin", remote.path())?;
    source.run_git(["push", "origin", "HEAD:refs/heads/main"])?;
    remote.set_head("main")?;

    let clone = TestRepo::clone_from(remote.path())?;

    source.mesh_stdout([
        "commit",
        "mesh-a",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Mesh A",
    ])?;
    source.mesh_stdout(["push"])?;
    clone.mesh_stdout(["fetch"])?;

    clone.mesh_stdout([
        "commit",
        "mesh-b",
        "--link",
        "file3.txt#L1-L5:file4.txt#L10-L15",
        "-m",
        "Mesh B",
    ])?;
    let clone_mesh_tip = clone.read_ref("refs/meshes/v1/mesh-b")?;
    let clone_link_id = clone.show_file(&clone_mesh_tip, "links")?;
    let clone_link_tip = clone.read_ref(&format!("refs/links/v1/{clone_link_id}"))?;
    clone.mesh_stdout(["push"])?;

    source.mesh_stdout(["fetch"])?;

    assert_eq!(source.read_ref("refs/meshes/v1/mesh-b")?, clone_mesh_tip);
    assert_eq!(
        source.read_ref(&format!("refs/links/v1/{clone_link_id}"))?,
        clone_link_tip
    );

    Ok(())
}
