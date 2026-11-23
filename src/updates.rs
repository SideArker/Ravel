#[cfg(any(target_os = "linux", windows))]
use anyhow::ensure;
use anyhow::{Context, Result};
use semver::Version;
use serde::Deserialize;
#[cfg(any(target_os = "linux", windows))]
use std::process::Command;
use std::{path::PathBuf, time::Duration};

const LATEST_RELEASE: &str = "https://api.github.com/repos/SideArker/Ravel/releases/latest";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

pub fn check() -> Result<Option<Version>> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .new_agent();
    let mut response = agent
        .get(LATEST_RELEASE)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", concat!("Ravel/", env!("CARGO_PKG_VERSION")))
        .call()
        .context("GitHub release request failed")?;
    let release: Release = response
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_json()
        .context("Invalid GitHub release response")?;
    newer_tag(&release.tag_name, env!("CARGO_PKG_VERSION"))
}

fn newer_tag(tag: &str, current: &str) -> Result<Option<Version>> {
    let latest = Version::parse(tag.strip_prefix('v').unwrap_or(tag))
        .context("Invalid GitHub release version")?;
    let current = Version::parse(current).context("Invalid installed version")?;
    Ok((latest > current).then_some(latest))
}

pub fn release_url(version: &Version) -> String {
    format!("https://github.com/SideArker/Ravel/releases/tag/v{version}")
}

pub fn install(version: &Version) -> Result<()> {
    let current_exe =
        std::env::current_exe().context("Cannot determine current executable path")?;
    let parent = current_exe
        .parent()
        .context("Current executable has no parent directory")?;
    let tmp = tempfile::tempdir().context("Cannot create temporary update directory")?;

    #[cfg(target_os = "linux")]
    {
        let asset = "ravel-linux-x86_64.tar.gz";
        let url =
            format!("https://github.com/SideArker/Ravel/releases/download/v{version}/{asset}");
        let archive = tmp.path().join(asset);
        download_file(&url, &archive)?;
        let status = Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(tmp.path())
            .arg("ravel")
            .status()
            .context("Failed to run tar to extract update")?;
        ensure!(status.success(), "Failed to extract Linux release archive");
        let extracted = tmp.path().join("ravel");
        ensure!(
            extracted.is_file(),
            "Extracted archive did not contain ravel binary"
        );

        let staged = parent.join(format!(".ravel.update.{}", std::process::id()));
        std::fs::copy(&extracted, &staged)
            .with_context(|| format!("Cannot write update to {}", parent.display()))?;
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
        }
        if let Err(e) = std::fs::rename(&staged, &current_exe) {
            let _ = std::fs::remove_file(&staged);
            return Err(e).context("Cannot replace executable with updated binary");
        }
        Ok(())
    }

    #[cfg(windows)]
    {
        let asset = "ravel-windows-x86_64.zip";
        let url =
            format!("https://github.com/SideArker/Ravel/releases/download/v{version}/{asset}");
        let archive = tmp.path().join(asset);
        download_file(&url, &archive)?;
        let status = Command::new("tar.exe")
            .arg("-xf")
            .arg(&archive)
            .arg("-C")
            .arg(tmp.path())
            .status()
            .context("Failed to extract Windows release archive")?;
        ensure!(status.success(), "Failed to unpack Windows release ZIP");
        let extracted = tmp.path().join("ravel.exe");
        ensure!(
            extracted.is_file(),
            "Extracted archive did not contain ravel.exe"
        );

        let old_exe = parent.join("ravel.exe.old");
        let _ = std::fs::remove_file(&old_exe);
        std::fs::rename(&current_exe, &old_exe)
            .context("Cannot move current ravel.exe aside for update")?;
        if let Err(e) = std::fs::copy(&extracted, &current_exe) {
            let _ = std::fs::rename(&old_exe, &current_exe);
            return Err(e).context("Cannot copy updated ravel.exe into place");
        }
        Ok(())
    }

    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (version, parent, tmp);
        anyhow::bail!("Automatic self-update is supported on Linux and Windows only")
    }
}

#[cfg(any(target_os = "linux", windows))]
fn download_file(url: &str, dest: &std::path::Path) -> Result<()> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .new_agent();
    let mut response = agent
        .get(url)
        .header("User-Agent", concat!("Ravel/", env!("CARGO_PKG_VERSION")))
        .call()
        .context("Failed to download release archive")?;
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .reader();
    let mut file = std::fs::File::create(dest)?;
    std::io::copy(&mut reader, &mut file)?;
    Ok(())
}

fn autostart_path() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(config.join("autostart").join("ravel.desktop"))
    }
    #[cfg(windows)]
    {
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from)?;
        Some(
            appdata
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Startup")
                .join("Ravel.lnk"),
        )
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        Some(
            home.join("Library")
                .join("LaunchAgents")
                .join("com.sidearker.ravel.plist"),
        )
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        None
    }
}

pub fn is_autostart_enabled() -> bool {
    autostart_path().is_some_and(|p| p.is_file())
}

pub fn set_autostart(enabled: bool) -> Result<()> {
    let path = autostart_path().context("Autostart path unavailable on this OS")?;
    if !enabled {
        if path.exists() {
            std::fs::remove_file(&path).context("Cannot remove autostart entry")?;
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("Cannot create autostart directory")?;
    }
    let exe = std::env::current_exe().context("Cannot determine executable path")?;

    #[cfg(target_os = "linux")]
    {
        let escaped = exe
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        let content = format!(
            "[Desktop Entry]\nType=Application\nName=Ravel\nComment=Share a mouse and keyboard over your LAN\nExec=\"{escaped}\"\nIcon=ravel\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        );
        std::fs::write(&path, content).context("Cannot write autostart desktop entry")?;
        Ok(())
    }

    #[cfg(windows)]
    {
        let exe_str = exe.to_string_lossy().replace('\'', "''");
        let lnk_str = path.to_string_lossy().replace('\'', "''");
        let work_dir = exe
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_string_lossy()
            .replace('\'', "''");
        let script = format!(
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{lnk_str}');$s.TargetPath='{exe_str}';$s.WorkingDirectory='{work_dir}';$s.Description='Ravel LAN sharing';$s.Save()"
        );
        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .context("Failed to create Windows Startup shortcut")?;
        ensure!(
            status.success(),
            "Could not create Windows Startup shortcut"
        );
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        let exe_str = exe.to_string_lossy();
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>com.sidearker.ravel</string>\n<key>ProgramArguments</key><array><string>{exe_str}</string></array>\n<key>RunAtLoad</key><true/>\n</dict></plist>\n"
        );
        std::fs::write(&path, plist).context("Cannot write LaunchAgent plist")?;
        Ok(())
    }

    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = exe;
        anyhow::bail!("Autostart is unsupported on this OS")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_newer_semantic_versions_prompt() {
        assert_eq!(
            newer_tag("v0.2.0", "0.1.1").unwrap().unwrap().to_string(),
            "0.2.0"
        );
        assert!(newer_tag("v0.1.1", "0.1.1").unwrap().is_none());
        assert!(newer_tag("v0.1.0", "0.1.1").unwrap().is_none());
        assert!(newer_tag("vnot-a-version", "0.1.1").is_err());
        assert_eq!(
            release_url(&Version::parse("0.2.0").unwrap()),
            "https://github.com/SideArker/Ravel/releases/tag/v0.2.0"
        );
    }

    #[test]
    #[ignore = "requires GitHub network"]
    fn github_latest_release_is_valid() {
        check().unwrap();
    }
}
