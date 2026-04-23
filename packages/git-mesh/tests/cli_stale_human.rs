mod support;

use anyhow::Result;
use serde_json::Value;
use support::TestRepo;

#[test]
fn cli_stale_reports_drift() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Track ranges",
    ])?;

    test_repo.write_file(
        "file1.txt",
        "prefix\nline1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("shift file1 lines")?;

    let stale = test_repo.mesh_stdout(["stale", "my-mesh"])?;
    assert!(stale.contains("1 stale of 1 links"));
    assert!(stale.contains("MOVED"));
    assert!(stale.contains("file1.txt#L1-L5"));

    Ok(())
}

#[test]
fn cli_stale_includes_culprit_and_reconcile_data() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Track ranges",
    ])?;

    test_repo.write_file(
        "file1.txt",
        "line1\nline2\nupdated\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("modify file1")?;

    let human = test_repo.mesh_stdout(["stale", "my-mesh"])?;
    assert!(human.contains("caused by"));
    assert!(human.contains("modify file1"));
    assert!(human.contains("reconcile with:"));
    assert!(human.contains("git mesh commit my-mesh \\"));
    assert!(human.contains("    --unlink "));

    let porcelain = test_repo.mesh_stdout(["stale", "my-mesh", "--format=porcelain"])?;
    assert!(porcelain.contains("reconcile=git mesh commit my-mesh --unlink"));
    assert!(porcelain.contains("leftCulprit="));
    assert!(porcelain.contains("modify file1"));

    let json = test_repo.mesh_stdout(["stale", "my-mesh", "--format=json"])?;
    let payload: Value = serde_json::from_str(&json)?;
    // §10.4: LSP Diagnostic shape — reconcile and culprit live under `data`.
    assert_eq!(
        payload["links"][0]["data"]["reconcile_command"],
        "git mesh commit my-mesh --unlink file1.txt#L1-L5:file2.txt#L10-L15 --link file1.txt#L1-L5:file2.txt#L10-L15 -m \"...\""
    );
    assert_eq!(
        payload["links"][0]["data"]["sides"][0]["culprit"]["summary"],
        "modify file1"
    );

    Ok(())
}

#[test]
fn cli_stale_human_output_matches_doc_conventions() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Track ranges",
    ])?;

    test_repo.write_file(
        "file1.txt",
        "line1\nREWRITE_A\nREWRITE_B\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("rewrite middle of file1")?;

    let human = test_repo.mesh_stdout(["stale", "my-mesh"])?;

    // Per-side tree branches: left uses `├─`, right uses `└─`.
    assert!(human.contains("├─"), "missing ├─ branch in:\n{human}");
    assert!(human.contains("└─"), "missing └─ branch in:\n{human}");

    // Rewritten-lines hint appended for MODIFIED sides (range unchanged here).
    assert!(
        human.contains("lines rewritten)"),
        "expected '(N/M lines rewritten)' hint, got:\n{human}"
    );
    assert!(
        human.contains("file1.txt#L1-L5  (") && human.contains("lines rewritten)"),
        "expected MODIFIED side anchored range then hint, got:\n{human}"
    );

    // Culprit attribution carries a short sha + subject + relative date.
    assert!(human.contains("caused by "));
    assert!(human.contains("rewrite middle of file1"));
    assert!(
        human.contains(" ago)") || human.contains("just now)"),
        "expected relative-date suffix on culprit line, got:\n{human}"
    );

    // Reconcile command is wrapped across multiple lines with trailing `\`.
    assert!(human.contains("reconcile with:"));
    assert!(
        human.contains("git mesh commit my-mesh \\"),
        "expected wrapped reconcile head, got:\n{human}"
    );
    assert!(
        human.contains("    --unlink "),
        "expected indented --unlink continuation, got:\n{human}"
    );
    assert!(
        human.contains("    -m \"...\""),
        "expected indented -m continuation, got:\n{human}"
    );

    // Summary still mirrors `git status`'s header-then-body cadence.
    assert!(human.contains("1 stale of 1 links"));

    Ok(())
}

#[test]
fn cli_stale_moved_has_no_culprit_and_shows_moved_hint() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Track ranges",
    ])?;

    // Prepend lines to shift file1's range without modifying its contents.
    test_repo.write_file(
        "file1.txt",
        "prefix1\nprefix2\nline1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("shift file1 lines")?;

    let human = test_repo.mesh_stdout(["stale", "my-mesh"])?;
    assert!(human.contains("MOVED"));
    assert!(
        human.contains("(file unchanged, lines shifted)"),
        "expected MOVED hint, got:\n{human}"
    );
    assert!(
        human.contains("file1.txt#L1-L5 \u{2192} L3-L7"),
        "expected unicode arrow with range-only current, got:\n{human}"
    );
    assert!(
        !human.contains("caused by "),
        "MOVED must not carry a culprit, got:\n{human}"
    );

    Ok(())
}

#[test]
fn cli_stale_supports_stat_and_patch() -> Result<()> {
    let mut test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "my-mesh",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Track ranges",
    ])?;

    test_repo.write_file(
        "file1.txt",
        "line1\nline2\nupdated\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
    )?;
    test_repo.commit_all("modify file1")?;

    let stat = test_repo.mesh_stdout(["stale", "my-mesh", "--stat"])?;
    assert!(stat.contains("MODIFIED"));
    assert!(
        stat.contains("file1.txt#L1-L5:file2.txt#L10-L15 -> file1.txt#L1-L5:file2.txt#L10-L15")
    );
    assert!(!stat.contains("reconcile with:"));
    assert!(!stat.contains("├─"));

    let patch = test_repo.mesh_stdout(["stale", "my-mesh", "--patch"])?;
    assert!(patch.contains("--- file1.txt#L1-L5"));
    assert!(patch.contains("+++ file1.txt#L1-L5"));
    assert!(patch.contains("@@"));
    assert!(patch.contains("-line3"));
    assert!(patch.contains("+updated"));

    Ok(())
}
