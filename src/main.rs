#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
use anyhow::{Context, Result, bail};
use ravel::{
    gui::App,
    identity,
    runtime::{self, Worker},
};
use std::{path::PathBuf, time::Duration};

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
    let mut name = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "Ravel device".into());
    let mut port = 4243;
    let mut stop = false;
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
            "--name" => name = args.next().context("--name needs a device name")?,
            "--port" => port = args.next().context("--port needs a number")?.parse()?,
            "--stop" => stop = true,
            "--probe-backend" => probe = true,
            "--smoke-gui" => smoke = true,
            "--help" | "-h" => {
                println!(
                    "Ravel — LAN mouse & keyboard sharing\n\nOptions: --name NAME --port PORT --state-dir PATH --stop --probe-backend --smoke-gui\nSee README.md and docs/operation.md for pairing, backends and stop bindings."
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
    if probe {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(async {
            let mut backend =
                tokio::time::timeout(Duration::from_secs(3), ravel::input::Backend::new())
                    .await??;
            println!(
                "{}: {} × {} logical units (constructor check; no input injected)",
                backend.label, backend.display.width, backend.display.height
            );
            backend.close().await
        });
    }
    let worker = Worker::start(directory, name.clone(), port)?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([760., 680.])
            .with_min_inner_size([640., 560.]),
        ..Default::default()
    };
    eframe::run_native(
        "Ravel",
        options,
        Box::new(move |_| Ok(Box::new(App::new(worker, name, smoke)))),
    )
    .map_err(|e| anyhow::anyhow!("Cannot launch GUI: {e}"))
}
