use crate::{
    discovery::Peer,
    geometry::{Layout, Monitor, Side},
    identity::pretty_fingerprint,
    runtime::{Command, Event, Worker},
    store::TrustedPeer,
};
use eframe::egui::{self, Color32, RichText};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use zeroize::{Zeroize, Zeroizing};

struct Confirmation {
    fingerprint: String,
    name: String,
    deadline: Instant,
    reply: oneshot::Sender<bool>,
}
pub struct App {
    worker: Worker,
    name: String,
    status: String,
    backend: String,
    monitors: Vec<Monitor>,
    selected_monitor: Option<String>,
    port: u16,
    enabled: bool,
    peers: Vec<Peer>,
    trusted: BTreeMap<String, TrustedPeer>,
    address: String,
    pin: String,
    selected: Option<String>,
    layout: Layout,
    offer: Option<(Zeroizing<String>, Instant)>,
    confirmations: Vec<Confirmation>,
    exit_at: Option<Instant>,
}
impl App {
    pub fn new(worker: Worker, name: String, smoke: bool) -> Self {
        Self {
            worker,
            name,
            status: "Starting LAN service…".into(),
            backend: "Backend has not been checked. Check it before enabling sharing.".into(),
            monitors: vec![],
            selected_monitor: None,
            port: 0,
            enabled: false,
            peers: vec![],
            trusted: BTreeMap::new(),
            address: String::new(),
            pin: String::new(),
            selected: None,
            layout: Layout::default(),
            offer: None,
            confirmations: vec![],
            exit_at: smoke.then(|| Instant::now() + Duration::from_secs(3)),
        }
    }
    fn command(&mut self, command: Command) {
        if let Err(e) = self.worker.command(command) {
            self.status = e.to_string();
        }
    }
    fn update_events(&mut self) {
        while let Ok(event) = self.worker.events.try_recv() {
            match event {
                Event::Status(value) => self.status = value,
                Event::Backend(value) => self.backend = value,
                Event::Monitors(value) => self.monitors = value,
                Event::SelectedMonitor(value) => self.selected_monitor = value,
                Event::Peers(peers) => self.peers = peers,
                Event::Trusted(peers) => {
                    self.trusted = peers;
                    if let Some(fp) = &self.selected
                        && let Some(peer) = self.trusted.get(fp)
                    {
                        self.layout = peer.layout;
                    }
                }
                Event::Pin { value, deadline } => self.offer = Some((value, deadline)),
                Event::PinClosed => self.offer = None,
                Event::Confirm {
                    fingerprint,
                    name,
                    deadline,
                    reply,
                } => self.confirmations.push(Confirmation {
                    fingerprint,
                    name,
                    deadline,
                    reply,
                }),
                Event::Listening(port) => self.port = port,
                Event::Enabled(enabled) => self.enabled = enabled,
            }
        }
        if self
            .offer
            .as_ref()
            .is_some_and(|(_, t)| Instant::now() >= *t)
        {
            self.offer = None;
        }
        self.confirmations
            .retain(|c| Instant::now() < c.deadline && !c.reply.is_closed());
    }
    fn select(&mut self, fp: String, address: Option<String>) {
        self.layout = self.trusted.get(&fp).map(|p| p.layout).unwrap_or_default();
        self.selected = Some(fp);
        self.address = address.unwrap_or_default();
    }

    fn show_devices(&mut self, ui: &mut egui::Ui) {
        ui.heading("Devices");
        if self.trusted.is_empty() && self.peers.is_empty() {
            ui.label("No devices found yet. Pair a device or enter its address below.");
        }

        for (fp, peer) in self.trusted.clone() {
            let address = self
                .peers
                .iter()
                .find(|found| found.fingerprint == fp)
                .and_then(|found| found.addresses.first().map(ToString::to_string));
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.selected.as_ref() == Some(&fp), &peer.name)
                    .clicked()
                {
                    self.select(fp.clone(), address.clone());
                }
                ui.weak(if address.is_some() {
                    "Paired · nearby"
                } else {
                    "Paired"
                });
            });
        }
        for peer in self.peers.clone() {
            if self.trusted.contains_key(&peer.fingerprint) {
                continue;
            }
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(
                        self.selected.as_ref() == Some(&peer.fingerprint),
                        &peer.name,
                    )
                    .clicked()
                {
                    self.select(
                        peer.fingerprint.clone(),
                        peer.addresses.first().map(ToString::to_string),
                    );
                }
                ui.weak("New · pair first");
            });
        }

        ui.add_space(6.);
        ui.horizontal(|ui| {
            ui.label("Address");
            ui.add(
                egui::TextEdit::singleline(&mut self.address)
                    .hint_text("192.168.1.20:4243")
                    .char_limit(255)
                    .desired_width(240.),
            );
            if self.selected.is_some() && ui.small_button("Clear selection").clicked() {
                self.selected = None;
            }
        });
        if !self.trusted.is_empty() || !self.peers.is_empty() {
            ui.small("Enter an address if the device is not nearby.");
        }
    }

    fn show_connection(&mut self, ui: &mut egui::Ui) {
        ui.heading("Connect");
        let selected_trusted = self
            .selected
            .as_ref()
            .is_some_and(|fp| self.trusted.contains_key(fp));
        let selected_new = self.selected.is_some() && !selected_trusted;
        if selected_new {
            ui.label("Pair this device first, then connect.");
        } else if self.address.is_empty() {
            ui.label("Choose a nearby paired device or enter its address.");
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.address.is_empty() && !selected_new,
                    egui::Button::new("Connect to device"),
                )
                .clicked()
            {
                self.command(Command::Connect {
                    address: self.address.clone(),
                    expected: self.selected.clone(),
                });
            }
            if !self.enabled && ui.button("Allow incoming control").clicked() {
                self.command(Command::Enable);
            }
        });
        ui.small("The receiving device must allow incoming control.");
    }

    fn show_pairing(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Pair a new device")
            .default_open(self.trusted.is_empty())
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Show PIN on this device").clicked() {
                        self.command(Command::OfferPin);
                    }
                    if self.offer.is_some() && ui.button("Cancel PIN").clicked() {
                        self.command(Command::CancelPin);
                        self.offer = None;
                    }
                });
                if let Some((value, deadline)) = &self.offer {
                    ui.label(RichText::new(value.as_str()).monospace().size(30.));
                    ui.small(format!(
                        "Expires in {}s · 5 attempts",
                        deadline.saturating_duration_since(Instant::now()).as_secs()
                    ));
                }
                ui.add_space(6.);
                ui.horizontal(|ui| {
                    ui.label("Or enter the other device's PIN");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.pin)
                            .password(true)
                            .char_limit(8)
                            .desired_width(100.),
                    );
                    if ui
                        .add_enabled(
                            !self.address.is_empty() && self.pin.len() == 8,
                            egui::Button::new("Pair using PIN"),
                        )
                        .clicked()
                    {
                        let pin = Zeroizing::new(self.pin.clone());
                        self.pin.zeroize();
                        self.command(Command::Pair {
                            address: self.address.clone(),
                            pin,
                            expected: self.selected.clone(),
                        });
                    }
                });
                ui.small("Both devices will ask you to compare and approve fingerprints.");
            });
    }

    fn show_handoff(&mut self, ui: &mut egui::Ui) {
        ui.heading("Handoff edge");
        ui.label("Choose the monitor and edge where the pointer crosses to the other device.");
        if self.monitors.is_empty() {
            if ui.button("Find monitors / check input").clicked() {
                self.command(Command::Probe);
            }
        } else {
            let label = self
                .selected_monitor
                .clone()
                .unwrap_or_else(|| format!("{} (default)", self.monitors[0].id));
            let monitors = self.monitors.clone();
            egui::ComboBox::from_label("This device's handoff monitor")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for monitor in monitors {
                        let display = monitor.display;
                        let text = format!(
                            "{} — {:.0}×{:.0} at ({:.0}, {:.0})",
                            monitor.id, display.width, display.height, display.x, display.y
                        );
                        if ui
                            .selectable_label(
                                self.selected_monitor.as_deref() == Some(&monitor.id),
                                text,
                            )
                            .clicked()
                        {
                            self.command(Command::Monitor(monitor.id));
                        }
                    }
                });
            ui.small("Monitor changes apply on the next connection.");
        }
        if self.backend.starts_with("Backend unavailable") {
            ui.colored_label(Color32::from_rgb(230, 100, 100), &self.backend);
        }

        if let Some(fp) = self
            .selected
            .clone()
            .filter(|fp| self.trusted.contains_key(fp))
        {
            ui.add_space(6.);
            ui.label("Other device is on the:");
            ui.horizontal(|ui| {
                for (side, label) in [
                    (Side::Left, "Left"),
                    (Side::Right, "Right"),
                    (Side::Top, "Top"),
                    (Side::Bottom, "Bottom"),
                ] {
                    ui.selectable_value(&mut self.layout.side, side, label);
                }
            });
            draw_layout(ui, self.layout);
            egui::CollapsingHeader::new("Fine tune alignment").show(ui, |ui| {
                ui.add(
                    egui::Slider::new(&mut self.layout.alignment, -0.9..=0.9)
                        .text("Offset")
                        .fixed_decimals(2),
                );
                ui.small("Positive moves the other device down or right.");
            });
            if ui.button("Save handoff edge").clicked() {
                self.command(Command::Layout {
                    fingerprint: fp,
                    layout: self.layout,
                });
            }
        } else {
            ui.small("Pair and select a device to set its handoff edge.");
        }
    }

    fn show_details(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Details & troubleshooting").show(ui, |ui| {
            ui.label(format!("This device: {} · TCP port {}", self.name, self.port));
            ui.label("This device's fingerprint:");
            ui.add(
                egui::Label::new(
                    RichText::new(pretty_fingerprint(&self.worker.identity.fingerprint))
                        .monospace(),
                )
                .selectable(true),
            );
            if let Some(fp) = self.selected.clone().filter(|fp| self.trusted.contains_key(fp)) {
                ui.label("Selected device's fingerprint:");
                ui.add(
                    egui::Label::new(RichText::new(pretty_fingerprint(&fp)).monospace())
                        .selectable(true),
                );
                if ui.button("Unpair selected device").clicked() {
                    self.worker.stop();
                    self.command(Command::Revoke(fp));
                    self.selected = None;
                    self.address.clear();
                }
            }
            ui.horizontal(|ui| {
                if ui.button("Check input backend").clicked() {
                    self.command(Command::Probe);
                }
                ui.label(&self.backend);
            });
            ui.small("Nearby names and addresses are unverified until pairing is approved.");
            ui.small("Emergency stop: Ctrl+Alt+Shift+Esc while sending, or run ravel --stop locally.");
            ui.small("Wayland requires wlroots protocols and a compositor stop binding. See docs/operation.md.");
        });
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.update_events();
        ctx.request_repaint_after(Duration::from_millis(100));
        if self.exit_at.is_some_and(|t| Instant::now() >= t) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(8.);
            ui.horizontal(|ui| {
                ui.heading("Ravel");
                ui.weak(format!("on {}", self.name));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Stop sharing").color(Color32::WHITE))
                                .fill(Color32::from_rgb(160, 45, 45)),
                        )
                        .clicked()
                    {
                        self.worker.stop();
                        self.enabled = false;
                        self.offer = None;
                    }
                });
            });
            let (summary, color) = if self.status.starts_with("Sending input") {
                (
                    "Controlling the other device",
                    Color32::from_rgb(100, 200, 130),
                )
            } else if self.status.starts_with("Receiving remote input") {
                (
                    "This device is being controlled",
                    Color32::from_rgb(100, 200, 130),
                )
            } else if self.status.starts_with("Connected") {
                ("Connected", Color32::from_rgb(100, 200, 130))
            } else if self.status.starts_with("Disconnected; retrying") {
                ("Reconnecting", Color32::from_rgb(230, 180, 90))
            } else if self.backend.starts_with("Backend unavailable")
                || self.status.contains("failed")
                || self.status.contains("unavailable")
                || self.status.starts_with("Cannot")
            {
                ("Needs attention", Color32::from_rgb(230, 100, 100))
            } else if self.enabled {
                ("Ready to connect", Color32::from_rgb(100, 200, 130))
            } else {
                ("Sharing is off", Color32::from_rgb(230, 180, 90))
            };
            ui.colored_label(color, RichText::new(summary).strong());
            if self.backend.starts_with("Backend unavailable")
                || (!self.status.starts_with("Sharing stopped")
                    && !self.status.starts_with("Sharing allowed"))
            {
                ui.small(if self.backend.starts_with("Backend unavailable") {
                    &self.backend
                } else {
                    &self.status
                });
            }
            ui.add_space(8.);
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(8.);
                let width = ui.available_width() - 24.;
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(width);
                    self.show_devices(ui);
                });
                ui.add_space(8.);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(width);
                    self.show_connection(ui);
                });
                ui.add_space(8.);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(width);
                    self.show_pairing(ui);
                });
                ui.add_space(8.);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(width);
                    self.show_handoff(ui);
                });
                ui.add_space(8.);
                self.show_details(ui);
            });
        });
        if let Some(c) = self.confirmations.first() {
            let mut answer = None;
            egui::Window::new("Verify the device fingerprint").collapsible(false).resizable(false).show(ctx,|ui|{
                ui.label(format!("The PIN exchange succeeded with {}.",c.name));ui.label("Compare this full fingerprint with the one under Details & troubleshooting on that device:");
                ui.add(egui::Label::new(RichText::new(pretty_fingerprint(&c.fingerprint)).monospace()).selectable(true));
                ui.label(format!("Approval expires in {}s",c.deadline.saturating_duration_since(Instant::now()).as_secs()));
                ui.horizontal(|ui|{if ui.button("Fingerprints match — trust device").clicked(){answer=Some(true);}
                    if ui.button("Reject").clicked(){answer=Some(false);}});
            });
            if let Some(answer) = answer {
                let c = self.confirmations.remove(0);
                let _ = c.reply.send(answer);
            }
        }
    }
}
impl Drop for App {
    fn drop(&mut self) {
        self.pin.zeroize();
        self.worker.stop();
    }
}
fn draw_layout(ui: &mut egui::Ui, layout: Layout) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(300., 140.), egui::Sense::hover());
    let center = rect.center();
    let size = egui::vec2(85., 55.);
    let delta = match layout.side {
        Side::Left => egui::vec2(-95., layout.alignment as f32 * 50.),
        Side::Right => egui::vec2(95., layout.alignment as f32 * 50.),
        Side::Top => egui::vec2(layout.alignment as f32 * 90., -65.),
        Side::Bottom => egui::vec2(layout.alignment as f32 * 90., 65.),
    };
    for (pos, label, color) in [
        (
            center - delta * 0.5,
            "This device",
            Color32::from_rgb(40, 90, 115),
        ),
        (center + delta * 0.5, "Peer", Color32::from_rgb(65, 110, 75)),
    ] {
        let r = egui::Rect::from_center_size(pos, size);
        ui.painter().rect_filled(r, 5., color);
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(13.),
            Color32::WHITE,
        );
    }
}
