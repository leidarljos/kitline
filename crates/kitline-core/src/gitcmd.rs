use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

pub fn git(repo: Option<&Path>, args: &[&str]) -> Result<Vec<u8>> {
    git_stdin(repo, args, None)
}

pub fn git_stdin(repo: Option<&Path>, args: &[&str], stdin: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut cmd = Command::new("git");
    // Inherited GIT_DIR beats a later -C, and GIT_CONFIG_COUNT can install
    // a trailer command. Drop every GIT_* variable, then set only the ones
    // this process means to set. The repo is named by -C or by the cwd.
    for (key, _) in std::env::vars_os() {
        let Some(name) = key.to_str() else {
            continue;
        };
        if name.starts_with("GIT_") {
            cmd.env_remove(&key);
        }
    }
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .arg("--no-pager");
    if let Some(repo) = repo {
        cmd.arg("-C").arg(repo);
    } else {
        // No repo was named. Stay off the cwd checkout so its trailer.*
        // config cannot run a command and replace the id.
        cmd.current_dir("/").env("GIT_CEILING_DIRECTORIES", "/");
    }
    cmd.args(args);
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| Error::Git(e.to_string()))?;
    if let Some(bytes) = stdin {
        let mut pipe = child
            .stdin
            .take()
            .ok_or_else(|| Error::Git("stdin was not piped".into()))?;
        std::io::Write::write_all(&mut pipe, bytes).map_err(|e| Error::Git(e.to_string()))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| Error::Git(e.to_string()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(Error::Git(format!("{}: {}", args.join(" "), err.trim())));
    }
    Ok(out.stdout)
}

pub fn git_text(repo: Option<&Path>, args: &[&str]) -> Result<String> {
    let out = git(repo, args)?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

pub struct TempDir {
    inner: tempfile::TempDir,
}

impl TempDir {
    pub fn new() -> Result<Self> {
        let inner = tempfile::TempDir::new().map_err(|e| Error::Git(e.to_string()))?;
        Ok(Self { inner })
    }

    pub fn path(&self) -> &Path {
        self.inner.path()
    }
}
