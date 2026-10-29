use crate::{
    discovery::Peer,
    geometry::{Layout, Side},
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
                Event::Peers(peers) => self.peers = peers,
                Event::Trusted(peers) => {
                    self.trusted = peers;
                    if let Some(fp) = &self.selected {
                        if let Some(peer) = self.trusted.get(fp) {
                            self.layout = peer.layout;
                        }
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
        if let Some(address) = address {
            self.address = address;
        }
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
                ui.label("Mouse & keyboard · local network");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("Stop / Disconnect").color(Color32::WHITE),
                            )
                            .fill(Color32::from_rgb(160, 45, 45)),
                        )
                        .clicked()
                    {
                        self.worker.stop();
                        self.enabled = false;
                        self.offer = None;
                    }
                    if !self.enabled && ui.button("Allow sharing").clicked() {
                        self.command(Command::Enable);
                    }
                });
            });
            ui.label(&self.status);
            ui.add_space(8.);
        });
        egui::CentralPanel::default().show(ctx,|ui|{egui::ScrollArea::vertical().show(ui,|ui|{
            ui.heading(&self.name);ui.label(format!("TLS listener: LAN IPv4 · TCP {}",self.port));
            ui.label("This device's fingerprint — compare this on the other screen:");
            ui.add(egui::Label::new(RichText::new(pretty_fingerprint(&self.worker.identity.fingerprint)).monospace()).selectable(true));
            ui.horizontal(|ui|{if ui.button("Check input backend").clicked(){self.command(Command::Probe);}ui.label(&self.backend);});
            ui.small("One active monitor per device. Wayland requires wlroots protocols and a compositor stop binding. See docs/operation.md.");
            ui.separator();ui.heading("Nearby devices");
            if self.peers.is_empty(){ui.label("Searching the LAN… You can also enter an address below.");}
            let peers=self.peers.clone();
            for peer in peers {
                ui.horizontal(|ui|{
                    let selected=self.selected.as_ref()==Some(&peer.fingerprint);
                    if ui.selectable_label(selected,&peer.name).clicked(){self.select(peer.fingerprint.clone(),peer.addresses.first().map(ToString::to_string));}
                    ui.label(if self.trusted.contains_key(&peer.fingerprint){"Known fingerprint hint"}else{"Unpaired"});
                    if let Some(address)=peer.addresses.first(){ui.monospace(address.to_string());}
                });
            }
            ui.small("Discovery names, fingerprint hints and addresses are untrusted. TLS verifies the actual device when connecting.");
            ui.separator();ui.heading("Pair or connect");
            ui.horizontal(|ui|{ui.label("Address");if ui.add(egui::TextEdit::singleline(&mut self.address).hint_text("192.168.1.20:4243").char_limit(255)).changed(){self.selected=None;}});
            ui.horizontal(|ui|{
                if ui.button("Show a five-minute PIN").clicked(){self.command(Command::OfferPin);}
                if self.offer.is_some() && ui.button("Cancel PIN").clicked(){self.command(Command::CancelPin);self.offer=None;}
            });
            if let Some((value,deadline))=&self.offer{
                ui.label(RichText::new(value.as_str()).monospace().size(30.));
                ui.label(format!("Enter this PIN on the other device. Expires in {}s. Five attempts maximum.",deadline.saturating_duration_since(Instant::now()).as_secs()));
            }
            ui.horizontal(|ui|{
                ui.label("Other device's PIN");ui.add(egui::TextEdit::singleline(&mut self.pin).password(true).char_limit(8).desired_width(100.));
                if ui.add_enabled(!self.address.is_empty() && self.pin.len()==8,egui::Button::new("Pair using PIN")).clicked(){
                    let pin=Zeroizing::new(self.pin.clone());self.pin.zeroize();self.command(Command::Pair{address:self.address.clone(),pin,expected:self.selected.clone()});
                }
                if ui.add_enabled(!self.address.is_empty(),egui::Button::new("Connect paired device")).clicked(){self.command(Command::Connect{address:self.address.clone(),expected:self.selected.clone()});}
            });
            ui.small("Pairing requires fingerprint approval on both screens. Allow sharing on the receiving device before connecting.");
            ui.separator();ui.heading("Paired devices & layout");
            let trusted=self.trusted.clone();
            for (fp,peer) in trusted {
                ui.horizontal(|ui|{
                    if ui.selectable_label(self.selected.as_ref()==Some(&fp),&peer.name).clicked(){self.select(fp.clone(),self.peers.iter().find(|p|p.fingerprint==fp).and_then(|p|p.addresses.first().map(ToString::to_string)));}
                    if ui.button("Revoke / Unpair").clicked(){self.worker.stop();self.command(Command::Revoke(fp.clone()));}
                });
                ui.add(egui::Label::new(RichText::new(pretty_fingerprint(&fp)).small().monospace()).selectable(true));
            }
            if self.selected.as_ref().is_some_and(|fp|self.trusted.contains_key(fp)){
                ui.label("Place the selected peer relative to this screen:");
                ui.horizontal(|ui|{for (side,label) in [(Side::Left,"Left"),(Side::Right,"Right"),(Side::Top,"Top"),(Side::Bottom,"Bottom")]{ui.selectable_value(&mut self.layout.side,side,label);}});
                ui.add(egui::Slider::new(&mut self.layout.alignment,-0.9..=0.9).text("Alignment offset (fraction of edge)").fixed_decimals(2));
                ui.small("Positive offset moves the peer down (left/right) or right (top/bottom). Only overlapping edge fractions transfer control.");
                draw_layout(ui,self.layout);
                if ui.button("Save layout for the next connection").clicked(){self.command(Command::Layout{fingerprint:self.selected.clone().unwrap_or_default(),layout:self.layout});}
            }
            ui.separator();ui.label("Emergency stop: Ctrl+Alt+Shift+Esc while sending, or run ravel --stop locally. Stop disables incoming control and automatic reconnect.");
        });});
        if let Some(c) = self.confirmations.first() {
            let mut answer = None;
            egui::Window::new("Verify the device fingerprint").collapsible(false).resizable(false).show(ctx,|ui|{
                ui.label(format!("The PIN exchange succeeded with {}.",c.name));ui.label("Compare this full fingerprint with the fingerprint shown locally on that device:");
                ui.add(egui::Label::new(RichText::new(pretty_fingerprint(&c.fingerprint)).monospace()).selectable(true));
                ui.label(format!("Approval expires in {}s",c.deadline.saturating_duration_since(Instant::now()).as_secs()));
                ui.horizontal(|ui|{if ui.button("Fingerprints match — trust device").clicked(){answer=Some(true);}if ui.button("Reject").clicked(){answer=Some(false);}});
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
