//! Staging + commit handlers — §6.2, §6.3, §6.4, §10.5.

use crate::cli::{parse_range_address, AddArgs, CommitArgs, ConfigArgs, MessageArgs, RmArgs, StatusArgs};
use crate::staging::StagedConfig;
use crate::types::CopyDetection;
use crate::{
    append_add, append_config, append_remove, commit_mesh, read_mesh, set_message, status_view,
};
use anyhow::{anyhow, Context, Result};

pub fn run_add(repo: &gix::Repository, args: AddArgs) -> Result<i32> {
    crate::validation::validate_mesh_name(&args.name)?;
    for addr in &args.ranges {
        let (path, s, e) = parse_range_address(addr)?;
        append_add(repo, &args.name, &path, s, e, args.at.as_deref())?;
    }
    Ok(0)
}

pub fn run_rm(repo: &gix::Repository, args: RmArgs) -> Result<i32> {
    for addr in &args.ranges {
        let (path, s, e) = parse_range_address(addr)?;
        append_remove(repo, &args.name, &path, s, e)?;
    }
    Ok(0)
}

pub fn run_message(repo: &gix::Repository, args: MessageArgs) -> Result<i32> {
    let body = if let Some(m) = args.m {
        m
    } else if let Some(f) = args.file {
        std::fs::read_to_string(&f).with_context(|| format!("failed to read {f}"))?
    } else if args.edit {
        return Err(anyhow!("--edit not supported in headless contexts"));
    } else {
        return Err(anyhow!("no message source (use -m / -F / --edit)"));
    };
    set_message(repo, &args.name, &body)?;
    Ok(0)
}

pub fn run_commit(repo: &gix::Repository, args: CommitArgs) -> Result<i32> {
    let name = args
        .name
        .ok_or_else(|| anyhow!("`git mesh commit <name>` requires a name"))?;
    commit_mesh(repo, &name)?;
    println!("updated refs/meshes/v1/{name}");
    Ok(0)
}

pub fn run_status(repo: &gix::Repository, args: StatusArgs) -> Result<i32> {
    if args.check {
        // Walk every mesh with non-empty staging.
        let names = crate::list_mesh_names(repo).unwrap_or_default();
        let mut drifted = false;
        // Also scan staging dir for mesh names not yet committed.
        let wd = crate::git::work_dir(repo)?;
        let dir = wd.join(".git").join("mesh").join("staging");
        let mut candidates = std::collections::BTreeSet::new();
        for n in names {
            candidates.insert(n);
        }
        if dir.exists() {
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                let fname = entry.file_name();
                let fn_str = fname.to_string_lossy();
                if !fn_str.contains('.') {
                    candidates.insert(fn_str.into_owned());
                }
            }
        }
        for c in candidates {
            let sv = status_view(repo, &c)?;
            if !sv.drift.is_empty() {
                drifted = true;
            }
        }
        return Ok(if drifted { 1 } else { 0 });
    }
    let name = args
        .name
        .ok_or_else(|| anyhow!("`git mesh status <name>` requires a name (or --check)"))?;
    let sv = status_view(repo, &name)?;
    println!("mesh: {}", sv.name);
    if let Some(msg) = &sv.staging.message {
        println!("message: {}", msg.trim());
    }
    for a in &sv.staging.adds {
        println!("add  {}#L{}-L{}", a.path, a.start, a.end);
    }
    for r in &sv.staging.removes {
        println!("remove  {}#L{}-L{}", r.path, r.start, r.end);
    }
    for f in &sv.drift {
        println!("drift  {}#L{}-L{}", f.path, f.start, f.end);
    }
    Ok(0)
}

pub fn run_config(repo: &gix::Repository, args: ConfigArgs) -> Result<i32> {
    // Read mesh config.
    let mesh = read_mesh(repo, &args.name)?;
    match (args.unset, args.key, args.value) {
        (Some(_unset), _, _) => Err(anyhow!("--unset not supported yet")),
        (None, None, _) => {
            let staging = crate::staging::read_staging(repo, &args.name).unwrap_or_default();
            let (staged_cd, staged_iw) = crate::staging::resolve_staged_config(
                &staging,
                (mesh.config.copy_detection, mesh.config.ignore_whitespace),
            );
            let cd_changed = staged_cd != mesh.config.copy_detection;
            let iw_changed = staged_iw != mesh.config.ignore_whitespace;
            println!(
                "{}copy-detection {}{}",
                if cd_changed { "* " } else { "" },
                cd_str(staged_cd),
                if cd_changed { " (staged)" } else { "" }
            );
            println!(
                "{}ignore-whitespace {}{}",
                if iw_changed { "* " } else { "" },
                staged_iw,
                if iw_changed { " (staged)" } else { "" }
            );
            Ok(0)
        }
        (None, Some(key), None) => {
            match key.as_str() {
                "copy-detection" => println!("{}", cd_str(mesh.config.copy_detection)),
                "ignore-whitespace" => println!("{}", mesh.config.ignore_whitespace),
                other => return Err(anyhow!("unknown config key `{other}`")),
            }
            Ok(0)
        }
        (None, Some(key), Some(value)) => {
            let entry = match key.as_str() {
                "copy-detection" => StagedConfig::CopyDetection(match value.as_str() {
                    "off" => CopyDetection::Off,
                    "same-commit" => CopyDetection::SameCommit,
                    "any-file-in-commit" => CopyDetection::AnyFileInCommit,
                    "any-file-in-repo" => CopyDetection::AnyFileInRepo,
                    _ => return Err(anyhow!("invalid copy-detection value `{value}`")),
                }),
                "ignore-whitespace" => StagedConfig::IgnoreWhitespace(match value.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => return Err(anyhow!("invalid ignore-whitespace value `{value}`")),
                }),
                other => return Err(anyhow!("unknown config key `{other}`")),
            };
            append_config(repo, &args.name, &entry)?;
            Ok(0)
        }
    }
}

fn cd_str(cd: CopyDetection) -> &'static str {
    match cd {
        CopyDetection::Off => "off",
        CopyDetection::SameCommit => "same-commit",
        CopyDetection::AnyFileInCommit => "any-file-in-commit",
        CopyDetection::AnyFileInRepo => "any-file-in-repo",
    }
}
