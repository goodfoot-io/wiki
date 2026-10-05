use std::fs;
use std::io::{self, Write as _};
use std::path::PathBuf;

fn main() {
    // `print_stdout` is denied crate-wide; a failed write here panics, which
    // fails the build — the right outcome for a build script.
    #[expect(
        clippy::disallowed_methods,
        reason = "cargo directives go to the build script's stdout, which is not the CLI's"
    )]
    let mut stdout = io::stdout().lock();
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let package_json_path = manifest_dir.join("package.json");
    writeln!(stdout, "cargo:rerun-if-changed={}", package_json_path.display())
        .expect("write cargo directive");

    let package_json = fs::read_to_string(&package_json_path).expect("read package.json");
    let parsed: serde_json::Value =
        serde_json::from_str(&package_json).expect("parse package.json");
    let version = parsed
        .get("version")
        .and_then(serde_json::Value::as_str)
        .expect("package.json version");

    writeln!(stdout, "cargo:rustc-env=WIKI_VERSION={version}").expect("write cargo directive");
    stdout.flush().expect("flush cargo directives");
}
