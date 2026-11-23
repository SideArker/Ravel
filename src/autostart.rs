use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub fn config_directory() -> Result<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("No user config directory")
}

pub fn unit_path() -> Result<PathBuf> {
    Ok(config_directory()?.join("systemd/user/ravel.service"))
}

pub fn enabled() -> bool {
    config_directory().is_ok_and(|p| {
        p.join("systemd/user/graphical-session.target.wants/ravel.service")
            .exists()
    })
}

pub fn service_unit(exe: &Path) -> Result<String> {
    let exe = exe
        .to_str()
        .context("Autostart executable path is not UTF-8")?;
    ensure!(!exe.contains(['\n', '\r', '\0']), "Invalid executable path");
    let escaped = exe
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
        .replace('$', "$$");
    Ok(format!(
        "[Unit]\nDescription=Ravel mouse and keyboard sharing\nAfter=graphical-session-pre.target\nPartOf=graphical-session.target\n\n[Service]\nType=simple\nExecStart=\"{escaped}\" --foreground\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=graphical-session.target\n"
    ))
}

fn systemctl(args: &[&str]) -> Result<()> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .context("Cannot run systemctl --user")?;
    ensure!(
        output.status.success(),
        "systemctl --user {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    let path = unit_path()?;
    let legacy = config_directory()?.join("autostart/ravel.desktop");
    if enabled {
        let installed = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin/ravel"));
        let exe = match installed {
            Some(path) if path.is_file() => path,
            _ => std::env::current_exe()?,
        };
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, service_unit(&exe)?)?;
        let names: Vec<&str> = [
            "WAYLAND_DISPLAY",
            "DISPLAY",
            "XDG_CURRENT_DESKTOP",
            "XDG_SESSION_TYPE",
        ]
        .into_iter()
        .filter(|name| std::env::var_os(name).is_some())
        .collect();
        if !names.is_empty() {
            let mut args = vec!["import-environment"];
            args.extend(names);
            systemctl(&args)?;
        }
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "ravel.service"])?;
    } else if path.exists() {
        // Disabling login startup must not terminate the current sharing session.
        systemctl(&["disable", "ravel.service"])?;
        std::fs::remove_file(path)?;
        systemctl(&["daemon-reload"])?;
    }
    if legacy.exists() {
        std::fs::remove_file(legacy)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unit_uses_foreground_graphical_lifecycle_and_escapes_paths() {
        let unit = service_unit(Path::new("/home/test/a \"b\"/%R/$ravel")).unwrap();
        assert!(unit.contains("ExecStart=\"/home/test/a \\\"b\\\"/%%R/$$ravel\" --foreground"));
        assert!(unit.contains("WantedBy=graphical-session.target"));
        assert!(unit.contains("PartOf=graphical-session.target"));
        assert!(!unit.contains("After=graphical-session.target\n"));
        assert!(service_unit(Path::new("/tmp/bad\npath")).is_err());
    }
}
