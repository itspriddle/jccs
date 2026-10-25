use std::{
    path::Path,
    process::{Command, Stdio},
};

/// Returns the checked-out branch in `dir` (or the current directory), or
/// `None` when detached, outside a repo, or `git` is unavailable.
pub fn branch(dir: Option<&Path>) -> Option<String> {
    let mut cmd = Command::new("git");
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }

    let output = cmd
        .args([
            "--no-optional-locks",
            "symbolic-ref",
            "--short",
            "-q",
            "HEAD",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let branch = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!branch.is_empty()).then_some(branch)
}
