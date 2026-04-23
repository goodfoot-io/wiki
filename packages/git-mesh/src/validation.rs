use anyhow::Result;

pub(crate) const RESERVED_MESH_NAMES: &[&str] = &[
    "commit", "rm", "mv", "restore", "stale", "fetch", "push", "doctor", "log", "help",
];

/// Validate a mesh name per §3.4 of `docs/git-mesh.md`.
///
/// Mesh names must be ref-legal single-component path segments (the rules
/// `git check-ref-format` applies to a ref component), and must not collide
/// with reserved subcommand names (§10.2). This is called at every create
/// or rename *destination* site; it never inspects existing refs.
pub fn validate_mesh_name(name: &str) -> Result<()> {
    anyhow::ensure!(!name.is_empty(), "mesh name must not be empty");
    anyhow::ensure!(
        !RESERVED_MESH_NAMES.contains(&name),
        "mesh name `{name}` is reserved"
    );
    anyhow::ensure!(
        !name.starts_with('-'),
        "mesh name `{name}` must not start with `-`"
    );
    anyhow::ensure!(
        !name.starts_with('.'),
        "mesh name `{name}` must not start with `.`"
    );
    anyhow::ensure!(
        !name.ends_with('.'),
        "mesh name `{name}` must not end with `.`"
    );
    anyhow::ensure!(
        !name.ends_with(".lock"),
        "mesh name `{name}` must not end with `.lock`"
    );
    anyhow::ensure!(name != "@", "mesh name `@` is not allowed");
    anyhow::ensure!(
        !name.contains(".."),
        "mesh name `{name}` must not contain `..`"
    );
    anyhow::ensure!(
        !name.contains("@{"),
        "mesh name `{name}` must not contain `@{{`"
    );

    for ch in name.chars() {
        if ch == '/' {
            anyhow::bail!("mesh name `{name}` must not contain `/` (single path component required)");
        }
        if ch.is_whitespace() {
            anyhow::bail!("mesh name `{name}` must not contain whitespace");
        }
        if ch.is_control() {
            anyhow::bail!("mesh name `{name}` must not contain control characters");
        }
        if matches!(ch, '~' | '^' | ':' | '?' | '*' | '[' | '\\') {
            anyhow::bail!("mesh name `{name}` must not contain `{ch}`");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_mesh_name;

    #[track_caller]
    fn reject(name: &str, needle: &str) {
        let err = validate_mesh_name(name)
            .err()
            .unwrap_or_else(|| panic!("expected `{name}` to be rejected"))
            .to_string();
        assert!(
            err.contains(needle),
            "error for `{name}` = `{err}` did not contain `{needle}`"
        );
    }

    #[test]
    fn accepts_ordinary_names() {
        for name in ["foo", "foo-bar", "foo_bar", "frontend-backend-sync", "v1.2", "a.b"] {
            validate_mesh_name(name).unwrap_or_else(|err| panic!("`{name}` rejected: {err}"));
        }
    }

    #[test]
    fn rejects_reserved_names() {
        reject("stale", "reserved");
        reject("commit", "reserved");
        reject("doctor", "reserved");
    }

    #[test]
    fn rejects_empty() {
        reject("", "empty");
    }

    #[test]
    fn rejects_leading_dash() {
        reject("-foo", "start with `-`");
    }

    #[test]
    fn rejects_slash() {
        reject("foo/bar", "/");
    }

    #[test]
    fn rejects_whitespace() {
        reject("foo bar", "whitespace");
        reject("foo\tbar", "whitespace");
    }

    #[test]
    fn rejects_control_characters() {
        reject("foo\u{7f}bar", "control");
        reject("foo\u{01}bar", "control");
    }

    #[test]
    fn rejects_git_metacharacters() {
        for (name, needle) in [
            ("foo~bar", "`~`"),
            ("foo^bar", "`^`"),
            ("foo:bar", "`:`"),
            ("foo?bar", "`?`"),
            ("foo*bar", "`*`"),
            ("foo[bar", "`[`"),
            ("foo\\bar", "`\\`"),
        ] {
            reject(name, needle);
        }
    }

    #[test]
    fn rejects_dot_rules() {
        reject(".foo", "start with `.`");
        reject("foo.", "end with `.`");
        reject("foo.lock", "`.lock`");
        reject("foo..bar", "`..`");
    }

    #[test]
    fn rejects_at_brace_and_at_alone() {
        reject("@", "`@`");
        reject("foo@{0}", "`@{`");
    }
}
