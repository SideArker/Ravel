#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
use anyhow::{Context, Result, bail};
use ravel::{
    gui::App,
    identity,
    runtime::{self, Worker},
};
use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("Ravel: {error:#}");
        #[cfg(windows)]
        unsafe {
            use windows::{
                Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
                core::{HSTRING, w},
            };
            MessageBoxW(
                None,
                &HSTRING::from(format!("{error:#}")),
                w!("Ravel"),
                MB_OK | MB_ICONERROR,
            );
        }
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let mut directory = None;
    let mut name = None;
    let mut port = 4243;
    let mut stop = false;
    let mut kill_all = false;
    let mut probe = false;
    let mut smoke = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--state-dir" => {
                directory = Some(PathBuf::from(
                    args.next().context("--state-dir needs a path")?,
                ))
            }
            "--name" => name = Some(args.next().context("--name needs a device name")?),
            "--port" => port = args.next().context("--port needs a number")?.parse()?,
            "--stop" => stop = true,
            "--kill-all" => kill_all = true,
            "--probe-backend" => probe = true,
            "--smoke-gui" => smoke = true,
            "--help" | "-h" => {
                println!(
                    "Ravel — LAN mouse & keyboard sharing\n\nOptions: --name NAME --port PORT --state-dir PATH --stop --kill-all --probe-backend --smoke-gui\nSee README.md and docs/operation.md for pairing, backends and stop bindings."
                );
                return Ok(());
            }
            _ => bail!("Unknown option: {arg}"),
        }
    }
    let directory = directory
        .map(Ok)
        .unwrap_or_else(identity::default_directory)?;
    if stop {
        return runtime::request_stop(&directory);
    }
    if kill_all {
        return runtime::request_shutdown(&directory);
    }
    if probe {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(async {
            let mut backend =
                tokio::time::timeout(Duration::from_secs(120), ravel::input::Backend::new())
                    .await??;
            println!(
                "{}: {} × {} logical units (constructor check; no input injected)",
                backend.label, backend.display.width, backend.display.height
            );
            backend.close().await
        });
    }
    if !smoke && std::env::var_os("RAVEL_DAEMON_CHILD").is_none() {
        return start_or_show_daemon(&directory);
    }
    let store = ravel::store::Store::load(&directory)?;
    let name = store
        .name()
        .map(str::to_owned)
        .or(name)
        .unwrap_or_else(default_device_name);
    let worker = match Worker::start(directory.clone(), name.clone(), port, None) {
        Ok(worker) => worker,
        Err(error) => {
            if runtime::request_show(&directory).is_ok() {
                return Ok(());
            }
            return Err(error);
        }
    };
    #[allow(unused_mut)] // Linux may select Xwayland for reversible tray hiding.
    let mut options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_app_id("ravel")
            .with_icon(ravel::gui::window_icon())
            .with_transparent(true)
            .with_inner_size([760., 680.])
            .with_min_inner_size([640., 560.]),
        ..Default::default()
    };
    #[cfg(target_os = "linux")]
    let can_hide = std::env::var_os("DISPLAY").is_some() && x11rb::connect(None).is_ok();
    #[cfg(not(target_os = "linux"))]
    let can_hide = true;
    #[cfg(target_os = "linux")]
    if can_hide {
        // Wayland/winit cannot hide and later remap a toplevel. Xwayland gives
        // this GUI a reversible hide operation while input still uses Wayland.
        options.event_loop_builder = Some(Box::new(|builder| {
            use winit::platform::x11::EventLoopBuilderExtX11;
            builder.with_x11();
        }));
    }
    eframe::run_native(
        "Ravel",
        options,
        Box::new(move |_| Ok(Box::new(App::new(worker, name, smoke, can_hide)))),
    )
    .map_err(|e| anyhow::anyhow!("Cannot launch GUI: {e}"))
}

fn start_or_show_daemon(directory: &Path) -> Result<()> {
    match identity::lock_directory(directory) {
        Ok(lock) => drop(lock),
        Err(error) => {
            if runtime::request_show(directory).is_ok() {
                return Ok(());
            }
            return Err(error);
        }
    }
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(std::env::args_os().skip(1))
        .env("RAVEL_DAEMON_CHILD", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    let mut log_options = OpenOptions::new();
    log_options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
        log_options.mode(0o600);
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0000_0008); // DETACHED_PROCESS
    }
    command.stderr(Stdio::from(log_options.open(directory.join("daemon.log"))?));
    command.spawn()?;
    Ok(())
}

fn default_device_name() -> String {
    let raw = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .or_else(|_| {
            #[cfg(unix)]
            {
                let mut buf = [0u8; 256];
                let res = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
                if res == 0 {
                    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                    if let Ok(s) = std::str::from_utf8(&buf[..len]) {
                        return Ok(s.to_owned());
                    }
                }
                Err(())
            }
            #[cfg(not(unix))]
            {
                Err(())
            }
        });
    if let Ok(raw) = raw {
        let cleaned: String = raw
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(64)
            .collect();
        if !cleaned.is_empty() {
            return cleaned;
        }
    }
    "Ravel device".into()
}
