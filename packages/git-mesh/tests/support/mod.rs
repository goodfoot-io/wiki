#![allow(dead_code)]
#![allow(dead_code)]

use anyhow::{Result, anyhow};
use git_mesh::RangeSpec;
use std::fs;
use std::process::{Command, Output, Stdio};

pub struct BareRepo {
    pub dir: tempfile::TempDir,
}

pub struct TestRepo {
    pub repo: gix::Repository,
    pub dir: tempfile::TempDir,
}

impl BareRepo {
    pub fn new() -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let output = Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg(dir.path())
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "git init --bare failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(Self { dir })
    }

    pub fn path(&self) -> &std::path::Path {
        self.dir.path()
    }

    pub fn run_git<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let output = Command::new("git")
            .current_dir(self.dir.path())
            .args(args.into_iter().map(|arg| arg.as_ref().to_string()))
            .output()?;
        TestRepo::ensure_success(output, "git command failed")
    }

    pub fn git_output<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let output = self.run_git(args)?;
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    pub fn set_head(&self, branch: &str) -> Result<()> {
        self.git_output(["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")])?;
        Ok(())
    }
}

impl TestRepo {
    pub fn new() -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let repo = gix::init(dir.path())?;
        let mut test_repo = Self { repo, dir };

        test_repo.write_file("initial.txt", "initial content")?;
        test_repo.write_file(
            "file1.txt",
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
        )?;
        test_repo.write_file(
            "file2.txt",
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12\nline13\nline14\nline15\nline16\n",
        )?;
        test_repo.write_file(
            "file3.txt",
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n",
        )?;
        test_repo.write_file(
            "file4.txt",
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12\nline13\nline14\nline15\nline16\n",
        )?;
        test_repo.commit_all("initial commit")?;

        Ok(test_repo)
    }

    pub fn clone_from(path: &std::path::Path) -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let output = Command::new("git")
            .arg("clone")
            .arg(path)
            .arg(dir.path())
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let repo = gix::open(dir.path())?;
        Ok(Self { repo, dir })
    }

    pub fn write_file(&self, path: &str, content: &str) -> Result<()> {
        let p = self.dir.path().join(path);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(p, content)?;
        Ok(())
    }

    pub fn commit_all(&mut self, message: &str) -> Result<()> {
        self.run_git(["add", "."])?;
        self.run_git_with_identity(["commit", "-m", message])?;

        self.repo = gix::open(self.dir.path())?;
        Ok(())
    }

    fn git<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = Command::new("git");
        command.current_dir(self.dir.path());
        for arg in args {
            command.arg(arg.as_ref());
        }
        command
    }

    fn with_identity(command: &mut Command) -> &mut Command {
        command
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
    }

    fn ensure_success(output: Output, context: &str) -> Result<Output> {
        anyhow::ensure!(
            output.status.success(),
            "{context}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(output)
    }

    pub fn run_git<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self::ensure_success(self.git(args).output()?, "git command failed")
    }

    pub fn run_git_with_identity<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = self.git(args);
        Self::with_identity(&mut command);
        Self::ensure_success(command.output()?, "git command failed")
    }

    pub fn run_git_with_input<I, S>(&self, args: I, input: &str) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        use std::io::Write;

        let mut child = self
            .git(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| anyhow::anyhow!("missing stdin"))?;
            stdin.write_all(input.as_bytes())?;
        }
        let output = Self::ensure_success(child.wait_with_output()?, "git command failed")?;
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    pub fn git_output<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let output = self.run_git(args)?;
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    pub fn head_sha(&self) -> Result<String> {
        self.git_output(["rev-parse", "HEAD"])
    }

    pub fn add_remote(&self, name: &str, path: &std::path::Path) -> Result<()> {
        self.run_git(["remote", "add", name, &path.to_string_lossy()])?;
        Ok(())
    }

    pub fn set_config(&self, key: &str, value: &str) -> Result<()> {
        self.run_git(["config", key, value])?;
        Ok(())
    }

    pub fn config_get(&self, key: &str) -> Result<String> {
        self.git_output(["config", "--get", key])
    }

    pub fn config_get_all(&self, key: &str) -> Result<Vec<String>> {
        let output = self.git(["config", "--get-all", key]).output()?;
        match output.status.code() {
            Some(0) => {}
            Some(1) => return Ok(Vec::new()),
            _ => {
                return Err(anyhow!(
                    "git command failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
        Ok(String::from_utf8(output.stdout)?
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    pub fn write_blob(&self, content: &str) -> Result<String> {
        self.run_git_with_input(["hash-object", "-w", "--stdin"], content)
    }

    pub fn read_ref(&self, name: &str) -> Result<String> {
        self.git_output(["rev-parse", name])
    }

    pub fn show_file(&self, revision: &str, path: &str) -> Result<String> {
        self.git_output(["show", &format!("{revision}:{path}")])
    }

    pub fn commit_parents(&self, revision: &str) -> Result<Vec<String>> {
        Ok(self
            .git_output(["show", "-s", "--format=%P", revision])?
            .split_whitespace()
            .map(str::to_string)
            .collect())
    }

    pub fn set_ref(&mut self, name: &str, oid: &str) -> Result<()> {
        self.git_output(["update-ref", name, oid])?;
        self.repo = gix::open(self.dir.path())?;
        Ok(())
    }

    pub fn file_blob_at_head(&self, path: &str) -> Result<String> {
        self.git_output(["rev-parse", &format!("HEAD:{path}")])
    }

    pub fn create_link_fixture(
        &mut self,
        id: &str,
        sides: [RangeSpec; 2],
    ) -> Result<(String, String, String)> {
        let anchor_sha = self.head_sha()?;
        let side_a_blob = self.file_blob_at_head(&sides[0].path)?;
        let side_b_blob = self.file_blob_at_head(&sides[1].path)?;
        let link_text = format!(
            "anchor {anchor_sha}\ncreated 2026-01-01T00:00:00Z\nside {} {} {} same-commit true\t{}\nside {} {} {} same-commit true\t{}\n",
            sides[0].start,
            sides[0].end,
            side_a_blob,
            sides[0].path,
            sides[1].start,
            sides[1].end,
            side_b_blob,
            sides[1].path
        );
        let blob_oid = self.write_blob(&link_text)?;
        self.set_ref(&format!("refs/links/v1/{id}"), &blob_oid)?;
        Ok((id.to_string(), blob_oid, link_text))
    }

    pub fn create_mesh_fixture(
        &mut self,
        name: &str,
        message: &str,
        link_ids: &[&str],
    ) -> Result<String> {
        let mut link_ids = link_ids.to_vec();
        link_ids.sort();
        link_ids.dedup();
        let mut links_text = String::new();
        for id in link_ids {
            links_text.push_str(id);
            links_text.push('\n');
        }
        let links_blob = self.write_blob(&links_text)?;
        let tree_oid =
            self.run_git_with_input(["mktree"], &format!("100644 blob {links_blob}\tlinks\n"))?;

        let parent = self.read_ref(&format!("refs/meshes/v1/{name}")).ok();
        let mut commit_args = vec![
            "commit-tree".to_string(),
            tree_oid.clone(),
            "-m".to_string(),
            message.to_string(),
        ];
        if let Some(parent) = parent {
            commit_args.push("-p".to_string());
            commit_args.push(parent);
        }

        let commit_oid = self.run_git_with_identity(commit_args.iter().map(String::as_str))?;
        let commit_oid = String::from_utf8(commit_oid.stdout)?.trim().to_string();
        self.set_ref(&format!("refs/meshes/v1/{name}"), &commit_oid)?;
        Ok(commit_oid)
    }

    pub fn remove_file(&mut self, path: &str) -> Result<()> {
        fs::remove_file(self.dir.path().join(path))?;
        self.repo = gix::open(self.dir.path())?;
        Ok(())
    }

    pub fn rename_file(&mut self, from: &str, to: &str) -> Result<()> {
        fs::rename(self.dir.path().join(from), self.dir.path().join(to))?;
        self.repo = gix::open(self.dir.path())?;
        Ok(())
    }

    pub fn copy_file(&mut self, from: &str, to: &str) -> Result<()> {
        if let Some(parent) = self.dir.path().join(to).parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(self.dir.path().join(from), self.dir.path().join(to))?;
        self.repo = gix::open(self.dir.path())?;
        Ok(())
    }

    pub fn delete_ref(&mut self, name: &str) -> Result<()> {
        self.git_output(["update-ref", "-d", name])?;
        self.repo = gix::open(self.dir.path())?;
        Ok(())
    }

    pub fn ref_exists(&self, name: &str) -> bool {
        self.read_ref(name).is_ok()
    }

    pub fn list_refs(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .git_output(["for-each-ref", "--format=%(refname)", prefix])?
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    pub fn run_mesh<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.run_mesh_with_env(args, std::iter::empty::<(&str, &str)>())
    }

    pub fn run_mesh_with_env<I, S, E, K, V>(&self, args: I, envs: E) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
        E: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut command = Command::new(env!("CARGO_BIN_EXE_git-mesh"));
        command.current_dir(self.dir.path());
        for arg in args {
            command.arg(arg.as_ref());
        }
        for (key, value) in envs {
            command.env(key.as_ref(), value.as_ref());
        }
        command.output().map_err(Into::into)
    }

    pub fn mesh_stdout<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let output = self.run_mesh(args)?;
        anyhow::ensure!(
            output.status.success(),
            "git-mesh command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).map_err(Into::into)
    }

    pub fn mesh_stderr<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let output = self.run_mesh(args)?;
        if output.status.success() {
            return Err(anyhow!("git-mesh command unexpectedly succeeded"));
        }
        String::from_utf8(output.stderr).map_err(Into::into)
    }

    pub fn mesh_output<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.run_mesh(args)
    }

    pub fn mesh_stdout_with_env<I, S, E, K, V>(&self, args: I, envs: E) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
        E: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let output = self.run_mesh_with_env(args, envs)?;
        anyhow::ensure!(
            output.status.success(),
            "git-mesh command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).map_err(Into::into)
    }
}
