use anyhow::Result;
#[cfg(any(target_os = "linux", windows))]
use anyhow::{Context, ensure};
use std::path::Path;
#[cfg(any(target_os = "linux", windows))]
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[cfg(any(target_os = "linux", windows))]
pub fn uninstall(directory: &Path) -> Result<()> {
    crate::updates::set_autostart(false)?;
    if crate::runtime::is_running(directory)? {
        crate::runtime::request_shutdown(directory)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while crate::runtime::is_running(directory)? {
            ensure!(
                Instant::now() < deadline,
                "Ravel did not exit; close it before uninstalling"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("No home directory")?;
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        let bin = std::env::var_os("RAVEL_INSTALL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/bin"));
        remove_linux_installation(&bin, &data)?;
    }
    #[cfg(windows)]
    schedule_windows_uninstall()?;
    #[cfg(any(target_os = "linux", windows))]
    println!(
        "Ravel uninstalled. Pairing and settings retained at {}",
        directory.display()
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn remove_linux_installation(bin: &Path, data: &Path) -> Result<()> {
    for path in [
        bin.join("ravel"),
        data.join("applications/ravel.desktop"),
        data.join("icons/hicolor/scalable/apps/ravel.svg"),
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("Cannot remove {}", path.display()));
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn schedule_windows_uninstall() -> Result<()> {
    use std::{os::windows::process::CommandExt, process::Command};
    let local = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .context("LOCALAPPDATA is unavailable")?;
    let installed = local.join("Programs/Ravel");
    let programs = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .context("APPDATA is unavailable")?
        .join("Microsoft/Windows/Start Menu/Programs");
    let script = windows_uninstall_script(&installed, &programs, std::process::id());
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(0x08000000)
        .spawn()
        .context("Cannot schedule Windows uninstall")?;
    Ok(())
}

#[cfg(windows)]
fn windows_uninstall_script(installed: &Path, programs: &Path, pid: u32) -> String {
    let quote = |p: &Path| p.to_string_lossy().replace('\'', "''");
    format!(
        r#"
$ErrorActionPreference = 'Stop'
$ravelProcess = Get-Process -Id {pid} -ErrorAction SilentlyContinue
if ($ravelProcess) {{ $ravelProcess.WaitForExit() }}
foreach ($ravelFile in @('{dir}\ravel.exe', '{dir}\ravel.exe.old', '{programs}\Ravel.lnk')) {{
    if (Test-Path -LiteralPath $ravelFile) {{ Remove-Item -LiteralPath $ravelFile -Force }}
}}
$ravelUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$ravelEntries = @($ravelUserPath -split ';' | Where-Object {{ $_ -and $_.TrimEnd('\') -ine '{dir}' }})
[Environment]::SetEnvironmentVariable('Path', ($ravelEntries -join ';'), 'User')
if ((Test-Path -LiteralPath '{dir}') -and -not (Get-ChildItem -LiteralPath '{dir}' -Force)) {{ [IO.Directory]::Delete('{dir}') }}
"#,
        pid = pid,
        dir = quote(installed),
        programs = quote(programs)
    )
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    #[test]
    fn uninstall_script_parses_without_executing_file_removal() {
        let script = windows_uninstall_script(
            Path::new(r"C:\Users\O'Brien\Programs\Ravel"),
            Path::new(r"C:\Users\O'Brien\Start Menu\Programs"),
            123,
        );
        assert!(script.contains("O''Brien"));
        assert!(!script.contains("-Recurse"));
        let result = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command",
                "$tokens = $null; $errors = $null; [System.Management.Automation.Language.Parser]::ParseInput($env:RAVEL_UNINSTALL_SCRIPT, [ref]$tokens, [ref]$errors) > $null; if ($errors.Count) { $errors | Out-String | Write-Error; exit 1 }"])
            .env("RAVEL_UNINSTALL_SCRIPT", script)
            .output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn uninstall_removes_only_managed_files_and_never_follows_binary_symlink() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        let data = root.path().join("data");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(data.join("ravel")).unwrap();
        let source = root.path().join("development-binary");
        std::fs::write(&source, b"keep build").unwrap();
        std::os::unix::fs::symlink(&source, bin.join("ravel")).unwrap();
        let settings = data.join("ravel/peers.json");
        std::fs::write(&settings, b"keep trust").unwrap();
        std::fs::write(bin.join("other-app"), b"keep other app").unwrap();
        remove_linux_installation(&bin, &data).unwrap();
        assert!(!bin.join("ravel").exists());
        assert!(source.exists());
        assert_eq!(std::fs::read(settings).unwrap(), b"keep trust");
        assert!(bin.join("other-app").exists());
        remove_linux_installation(&bin, &data).unwrap();
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn uninstall(_: &Path) -> Result<()> {
    anyhow::bail!("Uninstall currently supports Linux and Windows")
}
