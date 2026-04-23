mod support;

use anyhow::Result;
use support::TestRepo;

#[test]
fn cli_lists_and_shows_meshes() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "alpha",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Alpha subject",
    ])?;
    test_repo.mesh_stdout([
        "commit",
        "beta",
        "--link",
        "file3.txt#L1-L5:file4.txt#L10-L15",
        "-m",
        "Beta subject",
    ])?;

    let list = test_repo.mesh_stdout(std::iter::empty::<&str>())?;
    assert!(list.contains("alpha\t1 links\tAlpha subject"));
    assert!(list.contains("beta\t1 links\tBeta subject"));

    let show = test_repo.mesh_stdout(["alpha"])?;
    assert!(show.contains("mesh alpha"));
    assert!(show.contains("Alpha subject"));
    assert!(show.contains("Links (1):"));
    assert!(show.contains("  file1.txt#L1-L5:file2.txt#L10-L15"));
    assert!(!show.contains("full-link"));

    let show_oneline = test_repo.mesh_stdout(["alpha", "--oneline"])?;
    assert!(!show_oneline.contains("mesh alpha"));
    assert!(!show_oneline.contains("Author:"));
    assert!(!show_oneline.contains("Alpha subject"));
    assert!(show_oneline.contains("  file1.txt#L1-L5:file2.txt#L10-L15"));

    let formatted = test_repo.mesh_stdout(["alpha", "--format=%M %s%n%L%n%l links"])?;
    assert!(formatted.contains("alpha Alpha subject"));
    assert!(formatted.contains("  file1.txt#L1-L5:file2.txt#L10-L15"));
    assert!(formatted.contains("1 links"));

    // §10.2 — `%m` collides with git-log's left/right mark and is not defined
    // for mesh output. Format strings using `%m` must be rejected.
    let err = test_repo.mesh_stderr(["alpha", "--format=%m"])?;
    assert!(
        err.contains("unsupported format token `%m`"),
        "expected rejection of `%m`, got: {err}"
    );

    Ok(())
}

#[test]
fn cli_show_format_tokens_cover_git_log_compatible_set() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "tokens",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Subject line\n\nBody paragraph.",
    ])?;

    let head_commit = test_repo.read_ref("refs/meshes/v1/tokens")?;

    // Each git-log-compatible token round-trips the expected value.
    let h = test_repo.mesh_stdout(["tokens", "--format=%H"])?;
    assert_eq!(h.trim_end(), head_commit);

    let abbrev = test_repo.mesh_stdout(["tokens", "--format=%h"])?;
    assert_eq!(abbrev.trim_end().len(), 8);
    assert!(head_commit.starts_with(abbrev.trim_end()));

    let s = test_repo.mesh_stdout(["tokens", "--format=%s"])?;
    assert_eq!(s.trim_end(), "Subject line");

    let b = test_repo.mesh_stdout(["tokens", "--format=%B"])?;
    assert!(b.contains("Subject line"));
    assert!(b.contains("Body paragraph."));

    let an = test_repo.mesh_stdout(["tokens", "--format=%an"])?;
    assert!(!an.trim().is_empty());

    let ae = test_repo.mesh_stdout(["tokens", "--format=%ae"])?;
    assert!(ae.contains('@'));

    let ad = test_repo.mesh_stdout(["tokens", "--format=%ad"])?;
    assert!(!ad.trim().is_empty());

    // Mesh-specific tokens.
    let m = test_repo.mesh_stdout(["tokens", "--format=%M"])?;
    assert_eq!(m.trim_end(), "tokens");

    let links = test_repo.mesh_stdout(["tokens", "--format=%L"])?;
    assert!(links.contains("file1.txt#L1-L5:file2.txt#L10-L15"));

    let count = test_repo.mesh_stdout(["tokens", "--format=%l"])?;
    assert_eq!(count.trim_end(), "1");

    // Literal escape + newline.
    let literal = test_repo.mesh_stdout(["tokens", "--format=a%%b%nc"])?;
    assert!(literal.contains("a%b"));
    assert!(literal.contains("\nc"));

    Ok(())
}

#[test]
fn cli_show_matches_spec_read_output_format() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "alpha",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Alpha subject",
    ])?;

    let show = test_repo.mesh_stdout(["alpha"])?;
    let mut lines = show.lines();
    assert_eq!(lines.next(), Some("mesh alpha"));
    let commit_line = lines.next().expect("commit line");
    // Per §10.4, commit is full sha by default (40 hex chars).
    let commit_sha = commit_line
        .strip_prefix("commit ")
        .expect("commit line prefix");
    assert_eq!(
        commit_sha.len(),
        40,
        "commit sha should be full length by default, got `{commit_sha}`"
    );
    assert!(commit_sha.chars().all(|c| c.is_ascii_hexdigit()));
    let author_line = lines.next().expect("author line");
    assert!(author_line.starts_with("Author: "));
    let date_line = lines.next().expect("date line");
    assert!(date_line.starts_with("Date:   "));
    assert_eq!(lines.next(), Some(""));
    assert_eq!(lines.next(), Some("    Alpha subject"));
    assert_eq!(lines.next(), Some(""));
    assert_eq!(lines.next(), Some("Links (1):"));
    let link_line = lines.next().expect("link line");
    // Format: "    <short-sha>  <rangeA>:<rangeB>" (4-space indent, 2-space gap).
    let rest = link_line
        .strip_prefix("    ")
        .expect("4-space indent on link line");
    let (short, pair) = rest
        .split_once("  ")
        .expect("two-space gap between short sha and pair");
    assert_eq!(short.len(), 8, "short sha is 8 chars by default");
    assert!(short.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(pair, "file1.txt#L1-L5:file2.txt#L10-L15");

    // --no-abbrev expands the link sha to 40 chars; commit already full.
    let show_full = test_repo.mesh_stdout(["alpha", "--no-abbrev"])?;
    let link_line_full = show_full
        .lines()
        .find(|l| l.contains("file1.txt#L1-L5:file2.txt#L10-L15"))
        .expect("link line in --no-abbrev output");
    let rest_full = link_line_full.strip_prefix("    ").expect("indent");
    let (short_full, _) = rest_full.split_once("  ").expect("gap");
    assert_eq!(short_full.len(), 40);

    Ok(())
}

#[test]
fn cli_show_supports_at_log_diff_and_no_abbrev() -> Result<()> {
    let test_repo = TestRepo::new()?;
    test_repo.mesh_stdout([
        "commit",
        "mesh-history",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "First state",
    ])?;
    test_repo.mesh_stdout([
        "commit",
        "mesh-history",
        "--link",
        "file3.txt#L1-L5:file4.txt#L10-L15",
        "-m",
        "Second state",
    ])?;

    let historical = test_repo.mesh_stdout(["mesh-history", "--at", "HEAD~1"])?;
    assert!(historical.contains("First state"));
    assert!(!historical.contains("file3.txt#L1-L5:file4.txt#L10-L15"));

    let no_abbrev = test_repo.mesh_stdout(["mesh-history", "--no-abbrev"])?;
    let head_commit = test_repo.read_ref("refs/meshes/v1/mesh-history")?;
    assert!(no_abbrev.contains(&format!("commit {head_commit}")));

    let log = test_repo.mesh_stdout(["mesh-history", "--log", "--limit", "1"])?;
    assert!(log.contains(&head_commit));
    assert!(log.contains("Second state"));
    assert!(!log.contains("First state"));

    let log_oneline = test_repo.mesh_stdout(["mesh-history", "--log", "--oneline"])?;
    assert!(log_oneline.contains("Second state"));
    assert!(log_oneline.contains("First state"));
    // §10.2 --oneline: one line per commit, no header/body.
    assert!(!log_oneline.contains("Author:"));
    assert!(!log_oneline.contains("Date:"));
    for line in log_oneline.lines().filter(|l| !l.is_empty()) {
        assert_eq!(
            line.matches('\n').count(),
            0,
            "oneline entries must not span multiple lines: {line:?}"
        );
    }

    let diff = test_repo.mesh_stdout(["mesh-history", "--diff", "HEAD~1..HEAD"])?;
    assert!(diff.contains("diff "));
    assert!(diff.contains("+ file3.txt#L1-L5:file4.txt#L10-L15 @"));
    assert!(!diff.contains("- file1.txt#L1-L5:file2.txt#L10-L15 @"));

    Ok(())
}

#[test]
fn cli_log_prints_full_commit_message_body() -> Result<()> {
    let test_repo = TestRepo::new()?;
    // §6.4: --log mirrors `git log refs/meshes/v1/<name>`, which prints the
    // full commit message (subject + body) indented four spaces.
    test_repo.mesh_stdout([
        "commit",
        "bodied",
        "--link",
        "file1.txt#L1-L5:file2.txt#L10-L15",
        "-m",
        "Subject here\n\nBody line one.\nBody line two.",
    ])?;

    let log = test_repo.mesh_stdout(["bodied", "--log"])?;
    assert!(log.contains("    Subject here"));
    assert!(
        log.contains("    Body line one."),
        "--log output must include body line one, got: {log}"
    );
    assert!(
        log.contains("    Body line two."),
        "--log output must include body line two, got: {log}"
    );

    let log_oneline = test_repo.mesh_stdout(["bodied", "--log", "--oneline"])?;
    assert!(log_oneline.contains("Subject here"));
    assert!(
        !log_oneline.contains("Body line one."),
        "--oneline must collapse to subject only, got: {log_oneline}"
    );

    Ok(())
}
