//! Resolve Python interpreter + package roots for workers.
//!
//! **User install:** packages live in the XDG data venv site-packages;
//! `python_root` is empty and `WorkerClient` omits `PYTHONPATH`.
//! **Dev:** repo `python/` on `PYTHONPATH` and/or editable `.venv`.

use crate::config::Config;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Resolve PYTHONPATH root for workers.
///
/// Priority: `YAPPER_PYTHON_ROOT` → config dir if present → repo `python/` only
/// for **dev** interpreters → empty (site-packages; user install).
pub fn resolve_python_root(cfg: &Config) -> PathBuf {
    let bin = resolve_python_bin(cfg);
    let install_venv = default_install_venv_python();
    let using_install_venv = paths_equal_file(
        Path::new(&bin),
        install_venv.as_deref().unwrap_or(Path::new("")),
    ) || path_is_data_venv_python(Path::new(&bin));
    let dev_repo = if using_install_venv {
        // User install: do not point PYTHONPATH at a compile-time checkout path.
        None
    } else {
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("python"))
    };
    resolve_python_root_with(
        &cfg.paths.python_root,
        std::env::var("YAPPER_PYTHON_ROOT").ok().as_deref(),
        dev_repo,
    )
}

/// Pure path pick for workers' PYTHONPATH. Empty result means "no PYTHONPATH".
pub fn resolve_python_root_with(
    config_root: &str,
    env_override: Option<&str>,
    dev_repo_python: Option<PathBuf>,
) -> PathBuf {
    if let Some(p) = env_override {
        let t = p.trim();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    let from_cfg = config_root.trim();
    if !from_cfg.is_empty() {
        let path = PathBuf::from(from_cfg);
        if path.is_dir() {
            return path;
        }
        // Stale checkout path: fall through to dev or empty.
    }
    if let Some(manifest) = dev_repo_python {
        if manifest.is_dir() {
            return manifest;
        }
    }
    // User install: empty → WorkerClient omits PYTHONPATH; packages live in venv.
    PathBuf::new()
}

/// Resolve the Python interpreter for workers.
///
/// Priority: `YAPPER_PYTHON` → config path if it exists → XDG install venv →
/// repo `.venv` (dev) → config string (may be `python3`).
pub fn resolve_python_bin(cfg: &Config) -> String {
    resolve_python_bin_with(
        &cfg.paths.python_bin,
        std::env::var("YAPPER_PYTHON").ok().as_deref(),
        default_install_venv_python(),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".venv/bin/python")),
    )
}

fn default_install_venv_python() -> Option<PathBuf> {
    let p = crate::config::default_data_dir().join("venv/bin/python");
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

fn paths_equal_file(a: &Path, b: &Path) -> bool {
    if b.as_os_str().is_empty() || !a.is_file() || !b.is_file() {
        return false;
    }
    a == b
}

fn path_is_data_venv_python(bin: &Path) -> bool {
    let venv_python = crate::config::default_data_dir().join("venv/bin/python");
    bin == venv_python.as_path()
}

/// Pure interpreter pick. `install_venv` / `dev_venv` are candidate files.
pub fn resolve_python_bin_with(
    config_bin: &str,
    env_override: Option<&str>,
    install_venv: Option<PathBuf>,
    dev_venv: Option<PathBuf>,
) -> String {
    if let Some(p) = env_override {
        let t = p.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    let from_cfg = config_bin.trim();
    if !from_cfg.is_empty() {
        let path = PathBuf::from(from_cfg);
        if path.is_file() {
            return path.to_string_lossy().into();
        }
    }
    if let Some(p) = install_venv {
        if p.is_file() {
            return p.to_string_lossy().into();
        }
    }
    if let Some(p) = dev_venv {
        if p.is_file() {
            return p.to_string_lossy().into();
        }
    }
    if !from_cfg.is_empty() {
        return from_cfg.to_string();
    }
    "python3".into()
}

/// Whether a worker package is available: source tree dir and/or importable.
pub fn worker_package_status(python_bin: &str, python_root: &str, package: &str) -> String {
    let root = python_root.trim();
    if !root.is_empty() {
        let dir = Path::new(root).join(package);
        if dir.is_dir() {
            return "ok (tree)".into();
        }
    }
    // Site-packages / installed layout: try import (mirrors WorkerClient::spawn).
    let mut cmd = Command::new(python_bin);
    cmd.args(["-c", &format!("import {package}")])
        .env("PYTHONUNBUFFERED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if !root.is_empty() {
        cmd.env("PYTHONPATH", root);
    }
    match cmd.output() {
        Ok(o) if o.status.success() => "ok (import)".into(),
        _ => "MISSING".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolve_python_root_finds_repo_via_dev_fallback() {
        let repo_python = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("python");
        let root = resolve_python_root_with("", None, Some(repo_python.clone()));
        assert!(
            root.join("yapper_stt").is_dir() || root.join("yapper_common").is_dir(),
            "python root should contain packages: {}",
            root.display()
        );
        assert_eq!(root, repo_python);
    }

    #[test]
    fn resolve_python_root_empty_when_no_dev_and_no_config() {
        let root = resolve_python_root_with("", None, None);
        assert!(
            root.as_os_str().is_empty(),
            "installed layout must not invent a checkout path"
        );
    }

    #[test]
    fn resolve_python_root_prefers_env_over_config() {
        let root =
            resolve_python_root_with("/tmp/should-not-win", Some("/tmp/env-python-root"), None);
        assert_eq!(root, PathBuf::from("/tmp/env-python-root"));
    }

    #[test]
    fn resolve_python_root_uses_config_dir_when_present() {
        let dir = std::env::temp_dir().join(format!(
            "yapper-pyroot-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let root = resolve_python_root_with(dir.to_str().unwrap(), None, None);
        assert_eq!(root, dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_python_root_skips_missing_config_uses_dev() {
        let dev = std::env::temp_dir().join(format!(
            "yapper-dev-py-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dev).unwrap();
        let root =
            resolve_python_root_with("/nonexistent/yapper-python-root", None, Some(dev.clone()));
        assert_eq!(root, dev);
        let _ = fs::remove_dir_all(&dev);
    }

    #[test]
    fn resolve_python_bin_prefers_existing_config_path() {
        let dir = std::env::temp_dir().join(format!(
            "yapper-pybin-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("python");
        fs::write(&bin, b"#!/bin/true\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&bin).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&bin, perms).unwrap();
        }
        let got = resolve_python_bin_with(
            bin.to_str().unwrap(),
            None,
            Some(PathBuf::from("/no/install/venv")),
            Some(PathBuf::from("/no/dev/venv")),
        );
        assert_eq!(PathBuf::from(&got), bin);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_python_bin_env_wins() {
        let got = resolve_python_bin_with("/tmp/cfg-python", Some("/opt/custom/python"), None, None);
        assert_eq!(got, "/opt/custom/python");
    }

    #[test]
    fn resolve_python_bin_falls_back_to_name() {
        let got = resolve_python_bin_with("python3.11", None, None, None);
        assert_eq!(got, "python3.11");
    }

    #[test]
    fn worker_package_status_tree_ok() {
        let dir = std::env::temp_dir().join(format!(
            "yapper-pkg-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("yapper_stt")).unwrap();
        let status = worker_package_status("python3", dir.to_str().unwrap(), "yapper_stt");
        assert_eq!(status, "ok (tree)");
        let _ = fs::remove_dir_all(&dir);
    }
}
