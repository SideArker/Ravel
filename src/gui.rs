use crate::{
    discovery::Peer,
    geometry::{Display, Layout, Monitor, Side},
    identity::pretty_fingerprint,
    runtime::{Command, Event, Worker},
    session::{Direction, Feedback, PeerConnection},
    store::TrustedPeer,
};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use semver::Version;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};
use zeroize::{Zeroize, Zeroizing};

const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(3600);

#[derive(Clone, Copy)]
enum TrayAction {
    Show,
    HideTray,
    Quit,
}

fn logo_rgba(size: u32) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let s = size as f32;
    for y in 0..size {
        for x in 0..size {
            let u = (x as f32 + 0.5) / s * 2.0 - 1.0;
            let v = (y as f32 + 0.5) / s * 2.0 - 1.0;

            // Rounded squircle mask
            let qx = u.abs() - 0.58;
            let qy = v.abs() - 0.58;
            let dist_bg = (qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0)) - 0.34;
            if dist_bg > 0.03 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let bg_alpha = ((0.03 - dist_bg) / 0.06).clamp(0.0, 1.0);

            // Two interwoven figure-8 / reef-knot loops (Thread A & Thread B)
            let mut d_a = f32::MAX;
            let mut d_b = f32::MAX;
            let steps = 48;
            for i in 0..=steps {
                let t = (i as f32 / steps as f32) * std::f32::consts::TAU;
                // Lemniscate-like woven knot tilted at +35 deg and -35 deg
                let denom = 1.0 + t.sin().powi(2) * 0.65;
                let lx = 0.62 * t.cos() / denom;
                let ly = 0.48 * t.sin() * t.cos() / denom;

                let (c1, s1) = (0.82_f32, 0.57_f32);
                let ax = lx * c1 - ly * s1 - 0.03;
                let ay = lx * s1 + ly * c1;
                d_a = d_a.min((u - ax).hypot(v - ay));

                let bx = lx * c1 + ly * s1 + 0.03;
                let by = -lx * s1 + ly * c1;
                d_b = d_b.min((u - bx).hypot(v - by));
            }

            let thickness = 0.135;
            let edge = 2.2 / s;
            let a_cov = ((thickness - d_a) / edge).clamp(0.0, 1.0);
            let b_cov = ((thickness - d_b) / edge).clamp(0.0, 1.0);

            let mut r = 24.0 + (1.0 - v) * 8.0;
            let mut g = 27.0 + (1.0 - v) * 10.0;
            let mut b = 42.0 + (1.0 - v) * 14.0;

            let color_a = [94.0 + (u + 1.0) * 10.0, 225.0 - (v + 1.0) * 40.0, 245.0];
            let color_b = [255.0, 138.0 - (v + 1.0) * 20.0, 120.0 + (u + 1.0) * 45.0];

            // Interleave over/under based on quadrant so threads visibly weave
            let a_on_top = u * v >= 0.0;
            if a_on_top {
                if b_cov > 0.0 {
                    r = r * (1.0 - b_cov) + color_b[0] * b_cov;
                    g = g * (1.0 - b_cov) + color_b[1] * b_cov;
                    b = b * (1.0 - b_cov) + color_b[2] * b_cov;
                }
                if a_cov > 0.0 {
                    r = r * (1.0 - a_cov) + color_a[0] * a_cov;
                    g = g * (1.0 - a_cov) + color_a[1] * a_cov;
                    b = b * (1.0 - a_cov) + color_a[2] * a_cov;
                }
            } else {
                if a_cov > 0.0 {
                    r = r * (1.0 - a_cov) + color_a[0] * a_cov;
                    g = g * (1.0 - a_cov) + color_a[1] * a_cov;
                    b = b * (1.0 - a_cov) + color_a[2] * a_cov;
                }
                if b_cov > 0.0 {
                    r = r * (1.0 - b_cov) + color_b[0] * b_cov;
                    g = g * (1.0 - b_cov) + color_b[1] * b_cov;
                    b = b * (1.0 - b_cov) + color_b[2] * b_cov;
                }
            }

            rgba.push(r.clamp(0.0, 255.0) as u8);
            rgba.push(g.clamp(0.0, 255.0) as u8);
            rgba.push(b.clamp(0.0, 255.0) as u8);
            rgba.push((bg_alpha * 255.0) as u8);
        }
    }
    rgba
}

pub fn window_icon() -> Arc<egui::IconData> {
    let size = 64;
    Arc::new(egui::IconData {
        rgba: logo_rgba(size),
        width: size,
        height: size,
    })
}

fn tray_icon() -> anyhow::Result<Icon> {
    let size = 32;
    Ok(Icon::from_rgba(logo_rgba(size), size, size)?)
}

/// Tokenize numbers and command letters from an SVG `<path d="...">` attribute.
fn tokenize_svg_path(d: &str) -> Vec<Result<char, f32>> {
    let mut out = Vec::new();
    let bytes = d.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_whitespace() || c == ',' {
            i += 1;
        } else if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
            out.push(Ok(c));
            i += 1;
        } else if c == '-' || c == '+' || c == '.' || c.is_ascii_digit() {
            let start = i;
            let mut seen_dot = c == '.';
            i += 1;
            if (c == '-' || c == '+') && i < bytes.len() && bytes[i] == b'.' {
                seen_dot = true;
                i += 1;
            }
            while i < bytes.len() {
                let b = bytes[i] as char;
                if b.is_ascii_digit() {
                    i += 1;
                } else if b == '.' && !seen_dot {
                    seen_dot = true;
                    i += 1;
                } else if (b == 'e' || b == 'E') && i > start {
                    i += 1;
                    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            if let Ok(val) = d[start..i].parse::<f32>() {
                out.push(Err(val));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Parse closed 2D contours from an SVG `<path d="...">` string (`M/m, L/l, H/h, V/v, C/c, Z/z`).
fn parse_svg_path_contours(d: &str) -> Vec<Vec<[f32; 2]>> {
    let tokens = tokenize_svg_path(d);
    let mut contours = Vec::new();
    let mut current: Vec<[f32; 2]> = Vec::new();
    let mut cur = [0.0_f32, 0.0_f32];
    let mut start = [0.0_f32, 0.0_f32];
    let mut cmd = 'M';
    let mut idx = 0;

    let next_num = |idx: &mut usize| -> Option<f32> {
        match tokens.get(*idx) {
            Some(Err(v)) => {
                *idx += 1;
                Some(*v)
            }
            _ => None,
        }
    };

    while idx < tokens.len() {
        if let Ok(c) = tokens[idx] {
            cmd = c;
            idx += 1;
            if cmd == 'Z' || cmd == 'z' {
                if current.len() >= 3 {
                    contours.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                cur = start;
                continue;
            }
        }
        match cmd {
            'M' | 'm' => {
                let (Some(x), Some(y)) = (next_num(&mut idx), next_num(&mut idx)) else {
                    break;
                };
                if current.len() >= 3 {
                    contours.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                cur = if cmd == 'm' {
                    [cur[0] + x, cur[1] + y]
                } else {
                    [x, y]
                };
                start = cur;
                current.push(cur);
                cmd = if cmd == 'm' { 'l' } else { 'L' };
            }
            'L' | 'l' => {
                let (Some(x), Some(y)) = (next_num(&mut idx), next_num(&mut idx)) else {
                    break;
                };
                cur = if cmd == 'l' {
                    [cur[0] + x, cur[1] + y]
                } else {
                    [x, y]
                };
                current.push(cur);
            }
            'H' | 'h' => {
                let Some(x) = next_num(&mut idx) else {
                    break;
                };
                cur[0] = if cmd == 'h' { cur[0] + x } else { x };
                current.push(cur);
            }
            'V' | 'v' => {
                let Some(y) = next_num(&mut idx) else {
                    break;
                };
                cur[1] = if cmd == 'v' { cur[1] + y } else { y };
                current.push(cur);
            }
            'C' | 'c' => {
                let (Some(x1), Some(y1), Some(x2), Some(y2), Some(x), Some(y)) = (
                    next_num(&mut idx),
                    next_num(&mut idx),
                    next_num(&mut idx),
                    next_num(&mut idx),
                    next_num(&mut idx),
                    next_num(&mut idx),
                ) else {
                    break;
                };
                let p0 = cur;
                let (p1, p2, p3) = if cmd == 'c' {
                    (
                        [p0[0] + x1, p0[1] + y1],
                        [p0[0] + x2, p0[1] + y2],
                        [p0[0] + x, p0[1] + y],
                    )
                } else {
                    ([x1, y1], [x2, y2], [x, y])
                };
                for step in 1..=8 {
                    let t = step as f32 / 8.0;
                    let u = 1.0 - t;
                    let bx = u * u * u * p0[0]
                        + 3.0 * u * u * t * p1[0]
                        + 3.0 * u * t * t * p2[0]
                        + t * t * t * p3[0];
                    let by = u * u * u * p0[1]
                        + 3.0 * u * u * t * p1[1]
                        + 3.0 * u * t * t * p2[1]
                        + t * t * t * p3[1];
                    current.push([bx, by]);
                }
                cur = p3;
            }
            _ => {
                idx += 1;
            }
        }
    }
    if current.len() >= 3 {
        contours.push(current);
    }
    contours
}

/// Rasterize an SVG icon (`<path d="...">` on a 16×16 viewBox) into a white-alpha `ColorImage`.
fn rasterize_svg_icon(svg: &str, out_size: usize) -> Option<egui::ColorImage> {
    let mut contours = Vec::new();
    let mut rest = svg;
    while let Some(p_idx) = rest.find("<path") {
        let after = &rest[p_idx + 5..];
        let Some(end_idx) = after.find('>') else {
            break;
        };
        let tag = &after[..end_idx];
        for marker in ["d=\"", "d='"] {
            if let Some(d_start) = tag.find(marker) {
                let quote = marker.as_bytes()[2] as char;
                let d_rest = &tag[d_start + 3..];
                if let Some(d_end) = d_rest.find(quote) {
                    contours.extend(parse_svg_path_contours(&d_rest[..d_end]));
                }
            }
        }
        rest = &after[end_idx + 1..];
    }
    if contours.is_empty() {
        return None;
    }

    let view_size = 16.0_f32;
    let mut pixels = vec![Color32::TRANSPARENT; out_size * out_size];
    let mut any_opaque = false;
    let samples = 4;

    for y in 0..out_size {
        for x in 0..out_size {
            let mut hits = 0u32;
            for sy in 0..samples {
                let py =
                    (y as f32 + (sy as f32 + 0.5) / samples as f32) / out_size as f32 * view_size;
                for sx in 0..samples {
                    let px = (x as f32 + (sx as f32 + 0.5) / samples as f32) / out_size as f32
                        * view_size;
                    let inside = contours.iter().any(|poly| {
                        let mut winding = 0i32;
                        let n = poly.len();
                        for i in 0..n {
                            let a = poly[i];
                            let b = poly[(i + 1) % n];
                            if (a[1] <= py && b[1] > py) || (b[1] <= py && a[1] > py) {
                                let t = (py - a[1]) / (b[1] - a[1]);
                                let ix = a[0] + t * (b[0] - a[0]);
                                if px < ix {
                                    winding += if b[1] > a[1] { 1 } else { -1 };
                                }
                            }
                        }
                        winding != 0
                    });
                    if inside {
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                let alpha = ((hits * 255) / (samples * samples) as u32) as u8;
                any_opaque = true;
                pixels[y * out_size + x] = Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
            }
        }
    }
    any_opaque.then(|| egui::ColorImage {
        size: [out_size, out_size],
        source_size: Vec2::splat(out_size as f32),
        pixels,
    })
}

/// Locate and rasterize the active Linux / FreeDesktop icon theme's `window-close` icon.
#[cfg(target_os = "linux")]
fn load_system_close_icon(size: usize) -> Option<egui::ColorImage> {
    use std::path::PathBuf;

    let mut themes = Vec::new();
    let config_root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    if let Some(cfg) = &config_root {
        for sub in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
            if let Ok(text) = std::fs::read_to_string(cfg.join(sub)) {
                for line in text.lines() {
                    if let Some(val) = line.trim().strip_prefix("gtk-icon-theme-name")
                        && let Some((_, rhs)) = val.split_once('=')
                    {
                        let name = rhs.trim().trim_matches('"').trim_matches('\'');
                        if !name.is_empty() && !themes.contains(&name.to_owned()) {
                            themes.push(name.to_owned());
                        }
                    }
                }
            }
        }
    }
    if themes.is_empty()
        && let Ok(out) = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "icon-theme"])
            .output()
        && out.status.success()
    {
        let name = String::from_utf8_lossy(&out.stdout)
            .trim()
            .trim_matches('\'')
            .trim_matches('"')
            .to_owned();
        if !name.is_empty() {
            themes.push(name);
        }
    }
    for fallback in [
        "Papirus-Dark",
        "Papirus",
        "Adwaita",
        "breeze-dark",
        "breeze",
    ] {
        if !themes.iter().any(|t| t == fallback) {
            themes.push(fallback.to_owned());
        }
    }

    let mut base_dirs = Vec::new();
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        base_dirs.push(PathBuf::from(data_home).join("icons"));
    } else if let Some(home) = std::env::var_os("HOME") {
        let h = PathBuf::from(home);
        base_dirs.push(h.join(".local/share/icons"));
        base_dirs.push(h.join(".icons"));
    }
    base_dirs.push(PathBuf::from("/usr/local/share/icons"));
    base_dirs.push(PathBuf::from("/usr/share/icons"));

    let rel_paths = [
        "16x16/symbolic/actions/window-close-symbolic.svg",
        "16x16/actions/window-close.svg",
        "symbolic/ui/window-close-symbolic.svg",
        "symbolic/actions/window-close-symbolic.svg",
        "actions/16/window-close-symbolic.svg",
        "actions/16/window-close.svg",
        "24x24/symbolic/actions/window-close-symbolic.svg",
        "24x24/actions/window-close.svg",
        "actions/24/window-close-symbolic.svg",
        "actions/24/window-close.svg",
    ];

    for theme in &themes {
        for base in &base_dirs {
            let root = base.join(theme);
            for rel in &rel_paths {
                let candidate = root.join(rel);
                if let Ok(svg) = std::fs::read_to_string(&candidate)
                    && let Some(img) = rasterize_svg_icon(&svg, size)
                {
                    return Some(img);
                }
            }
        }
    }
    None
}

struct Confirmation {
    fingerprint: String,
    name: String,
    deadline: Instant,
    reply: oneshot::Sender<bool>,
}
enum UpdateState {
    Checking(Receiver<Result<Option<Version>, String>>),
    Updating(Version, Receiver<Result<(), String>>),
    Updated(Version),
    Available(Version, Option<String>),
    Current,
    Failed(String),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum PairStep {
    None,
    Pin,
    Fingerprint,
}
pub struct App {
    worker: Worker,
    repaint_registered: bool,
    name: String,
    name_draft: String,
    status: String,
    backend: String,
    monitors: Vec<Monitor>,
    selected_monitor: Option<String>,
    port: u16,
    enabled: bool,
    controller: Option<Direction>,
    handoff: Option<(f32, bool, Instant)>,
    peers: Vec<Peer>,
    trusted: BTreeMap<String, TrustedPeer>,
    connections: BTreeMap<String, PeerConnection>,
    connecting: BTreeSet<String>,
    peer_addresses: BTreeMap<String, String>,
    address: String,
    pin: String,
    pairing_pending: bool,
    selected: Option<String>,
    pair_step: PairStep,
    layout: Layout,
    dragging: Option<(String, Pos2)>,
    offer: Option<(Zeroizing<String>, Instant)>,
    confirmations: Vec<Confirmation>,
    updates: UpdateState,
    last_update_check: Instant,
    dismissed_update: Option<Version>,
    show_update: bool,
    autostart_enabled: bool,
    show_autostart_prompt: bool,
    hide_tray: bool,
    exit_at: Option<Instant>,
    exited: bool,
    quitting: bool,
    had_connection: bool,
    user_interacted: bool,
    can_hide: bool,
    tray: Option<TrayIcon>,
    tray_failed: bool,
    tray_sender: mpsc::Sender<TrayAction>,
    tray_events: Receiver<TrayAction>,
    #[cfg(target_os = "linux")]
    close_icon: Option<Option<egui::TextureHandle>>,
}
impl App {
    pub fn new(worker: Worker, name: String, smoke: bool, can_hide: bool) -> Self {
        let (tray_sender, tray_events) = mpsc::channel();
        if !smoke {
            let _ = worker.command(Command::Probe);
        }
        let autostart_enabled = crate::updates::is_autostart_enabled();
        Self {
            worker,
            repaint_registered: false,
            name_draft: name.clone(),
            name,
            status: "Starting LAN service…".into(),
            backend: "Checking input backend…".into(),
            monitors: vec![],
            selected_monitor: None,
            port: 0,
            enabled: false,
            controller: None,
            handoff: None,
            peers: vec![],
            trusted: BTreeMap::new(),
            connections: BTreeMap::new(),
            connecting: BTreeSet::new(),
            peer_addresses: BTreeMap::new(),
            address: String::new(),
            pin: String::new(),
            pairing_pending: false,
            selected: None,
            pair_step: PairStep::None,
            layout: Layout::default(),
            dragging: None,
            offer: None,
            confirmations: vec![],
            updates: Self::start_update_check(),
            last_update_check: Instant::now(),
            dismissed_update: None,
            show_update: false,
            autostart_enabled,
            show_autostart_prompt: false,
            hide_tray: false,
            exit_at: smoke.then(|| Instant::now() + Duration::from_secs(3)),
            exited: false,
            quitting: false,
            had_connection: false,
            user_interacted: false,
            can_hide,
            tray: None,
            tray_failed: false,
            tray_sender,
            tray_events,
            #[cfg(target_os = "linux")]
            close_icon: None,
        }
    }
    fn ensure_tray(&mut self, ctx: &egui::Context) {
        if self.hide_tray {
            self.tray = None;
            return;
        }
        if self.tray.is_some() || self.tray_failed {
            return;
        }
        let build = || -> anyhow::Result<TrayIcon> {
            let menu = Menu::new();
            let open = MenuItem::new("Open Ravel", true, None);
            let hide_tray = MenuItem::new("Hide tray icon", true, None);
            let quit = MenuItem::new("Quit Ravel and stop sharing", true, None);
            menu.append(&open)?;
            menu.append(&hide_tray)?;
            menu.append(&quit)?;
            let icon = TrayIconBuilder::new()
                .with_icon(tray_icon()?)
                .with_tooltip("Ravel")
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .build()?;
            let sender = self.tray_sender.clone();
            let repaint = ctx.clone();
            TrayIconEvent::set_event_handler(Some(move |event| {
                if matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } | TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    }
                ) {
                    let _ = sender.send(TrayAction::Show);
                    repaint.request_repaint();
                }
            }));
            let sender = self.tray_sender.clone();
            let repaint = ctx.clone();
            let open_id = open.id().clone();
            let hide_id = hide_tray.id().clone();
            let quit_id = quit.id().clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let action = if event.id == open_id {
                    Some(TrayAction::Show)
                } else if event.id == hide_id {
                    Some(TrayAction::HideTray)
                } else if event.id == quit_id {
                    Some(TrayAction::Quit)
                } else {
                    None
                };
                if let Some(action) = action {
                    let _ = sender.send(action);
                    repaint.request_repaint();
                }
            }));
            Ok(icon)
        };
        match build() {
            Ok(icon) => {
                self.tray = Some(icon);
            }
            Err(error) => {
                self.tray_failed = true;
                self.status = format!("System tray unavailable: {error:#}");
            }
        }
    }
    fn show_window(ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
    fn hide_window(ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }
    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        self.worker.stop();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    fn restart_updated(&mut self, ctx: &egui::Context) {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe)
                .args(std::env::args_os().skip(1))
                .spawn();
            self.quit(ctx);
        }
    }
    fn start_update_check() -> UpdateState {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = crate::updates::check().map_err(|error| format!("{error:#}"));
            let _ = tx.send(result);
        });
        UpdateState::Checking(rx)
    }
    fn start_self_update(version: Version) -> UpdateState {
        let (tx, rx) = mpsc::channel();
        let v = version.clone();
        std::thread::spawn(move || {
            let result = crate::updates::install(&v).map_err(|error| format!("{error:#}"));
            let _ = tx.send(result);
        });
        UpdateState::Updating(version, rx)
    }
    fn command(&mut self, command: Command) {
        if let Err(e) = self.worker.command(command) {
            self.status = e.to_string();
        }
    }
    fn select(&mut self, fp: String, address: Option<String>) {
        self.layout = self.trusted.get(&fp).map(|p| p.layout).unwrap_or_default();
        if let Some(addr) = address.filter(|a| !a.is_empty()) {
            self.peer_addresses.insert(fp.clone(), addr.clone());
            self.address = addr;
        } else if let Some(known) = self.peer_addresses.get(&fp).cloned() {
            self.address = known;
        }
        self.selected = Some(fp);
    }
    fn peer_address(&self, fp: &str) -> Option<String> {
        self.peers
            .iter()
            .find(|found| found.fingerprint == fp)
            .and_then(|found| found.addresses.first().map(ToString::to_string))
            .or_else(|| self.peer_addresses.get(fp).cloned())
    }
    fn update_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.worker.events.try_recv() {
            match event {
                Event::Show => {
                    self.user_interacted = true;
                    Self::show_window(ctx);
                }
                Event::Name(name) => {
                    self.name_draft = name.clone();
                    self.name = name;
                }
                Event::Preferences {
                    hide_tray,
                    autostart_prompted,
                } => {
                    self.hide_tray = hide_tray;
                    if hide_tray {
                        self.tray = None;
                    }
                    self.autostart_enabled = crate::updates::is_autostart_enabled();
                    if !autostart_prompted && !self.autostart_enabled && self.exit_at.is_none() {
                        self.show_autostart_prompt = true;
                    }
                }
                Event::Status(value) => {
                    if value.starts_with("Pairing failed") {
                        self.pairing_pending = false;
                        self.pair_step = PairStep::Pin;
                    }
                    self.status = value;
                }
                Event::Backend(value) => self.backend = value,
                Event::Monitors(value) => self.monitors = value,
                Event::SelectedMonitor(value) => self.selected_monitor = value,
                Event::Peers(peers) => {
                    for peer in &peers {
                        if let Some(addr) = peer.addresses.first() {
                            self.peer_addresses
                                .insert(peer.fingerprint.clone(), addr.to_string());
                        }
                    }
                    self.peers = peers;
                }
                Event::Trusted(peers) => {
                    self.trusted = peers;
                    if self.selected.is_none()
                        && let Some(first_fp) = self.trusted.keys().next().cloned()
                    {
                        let addr = self.peer_address(&first_fp);
                        self.select(first_fp, addr);
                    }
                    if let Some(fp) = &self.selected
                        && let Some(peer) = self.trusted.get(fp)
                    {
                        self.layout = peer.layout;
                        if self.pair_step == PairStep::Fingerprint {
                            self.pairing_pending = false;
                            self.pair_step = PairStep::None;
                        }
                    }
                }
                Event::Connections(list) => {
                    let was_empty = self.connections.is_empty();
                    self.connections = list
                        .into_iter()
                        .map(|c| (c.fingerprint.clone(), c))
                        .collect();
                    if self.selected.is_none()
                        && let Some(first_fp) = self.connections.keys().next().cloned()
                    {
                        let addr = self.peer_address(&first_fp);
                        self.select(first_fp, addr);
                    }
                    if was_empty && !self.connections.is_empty() {
                        if !self.had_connection
                            && !self.user_interacted
                            && self.can_hide
                            && (self.tray.is_some() || self.hide_tray)
                        {
                            Self::hide_window(ctx);
                        }
                        self.had_connection = true;
                    } else if !was_empty && self.connections.is_empty() && self.had_connection {
                        Self::show_window(ctx);
                    }
                }
                Event::Connecting(list) => {
                    self.connecting = list.into_iter().collect();
                }
                Event::Pin { value, deadline } => self.offer = Some((value, deadline)),
                Event::PinClosed => self.offer = None,
                Event::Confirm {
                    fingerprint,
                    name,
                    deadline,
                    reply,
                } => {
                    Self::show_window(ctx);
                    self.pairing_pending = false;
                    let address = self
                        .peer_address(&fingerprint)
                        .or_else(|| (!self.address.is_empty()).then(|| self.address.clone()));
                    self.select(fingerprint.clone(), address);
                    self.pair_step = PairStep::Fingerprint;
                    self.confirmations.push(Confirmation {
                        fingerprint,
                        name,
                        deadline,
                        reply,
                    });
                }
                Event::Listening(port) => self.port = port,
                Event::Enabled(enabled) => self.enabled = enabled,
                Event::Exited(error) => {
                    if let Some(error) = error {
                        self.status = error;
                    }
                    self.exited = true;
                }
                Event::Feedback(feedback) => match feedback {
                    Feedback::Controller(direction) => self.controller = Some(direction),
                    Feedback::Handoff { progress, incoming } => {
                        self.handoff = Some((progress, incoming, Instant::now()));
                    }
                    Feedback::Disconnected => {
                        self.controller = None;
                        self.handoff = None;
                    }
                },
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

        if !matches!(
            self.updates,
            UpdateState::Checking(_) | UpdateState::Updating(_, _) | UpdateState::Updated(_)
        ) && self.exit_at.is_none()
            && self.last_update_check.elapsed() >= UPDATE_CHECK_INTERVAL
        {
            self.last_update_check = Instant::now();
            self.updates = Self::start_update_check();
        }
        let checked = match &self.updates {
            UpdateState::Checking(rx) => match rx.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Disconnected) => Some(Err("Update check stopped".into())),
                Err(TryRecvError::Empty) => None,
            },
            _ => None,
        };
        if let Some(result) = checked {
            self.updates = match result {
                Ok(Some(version)) => {
                    if self.dismissed_update.as_ref() != Some(&version) {
                        self.show_update = true;
                    }
                    Self::start_self_update(version)
                }
                Ok(None) => UpdateState::Current,
                Err(error) => UpdateState::Failed(error),
            };
        }
        let installed = match &self.updates {
            UpdateState::Updating(version, rx) => match rx.try_recv() {
                Ok(result) => Some((version.clone(), result)),
                Err(TryRecvError::Disconnected) => {
                    Some((version.clone(), Err("Update interrupted".into())))
                }
                Err(TryRecvError::Empty) => None,
            },
            _ => None,
        };
        if let Some((version, result)) = installed {
            self.updates = match result {
                Ok(()) => UpdateState::Updated(version),
                Err(error) => UpdateState::Available(version, Some(error)),
            };
        }
    }

    fn show_devices(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Devices");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.offer.is_some() {
                    if ui.button("Cancel PIN").clicked() {
                        self.user_interacted = true;
                        self.command(Command::CancelPin);
                        self.offer = None;
                    }
                } else if ui.button("Show PIN on this device").clicked() {
                    self.user_interacted = true;
                    self.command(Command::OfferPin);
                }
            });
        });

        if let Some((value, deadline)) = &self.offer {
            ui.add_space(4.);
            egui::Frame::canvas(ui.style())
                .inner_margin(8.)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(value.as_str())
                                .monospace()
                                .strong()
                                .size(22.)
                                .color(Color32::from_rgb(120, 220, 150)),
                        );
                        ui.weak(format!(
                            "{}s",
                            deadline.saturating_duration_since(Instant::now()).as_secs()
                        ));
                    });
                });
        }

        if self.pair_step == PairStep::Pin {
            ui.add_space(4.);
            egui::Frame::canvas(ui.style())
                .inner_margin(8.)
                .show(ui, |ui| {
                    let target_label = self
                        .selected
                        .as_ref()
                        .and_then(|fp| {
                            self.peers
                                .iter()
                                .find(|p| &p.fingerprint == fp)
                                .map(|p| p.name.clone())
                        })
                        .unwrap_or_else(|| self.address.clone());
                    ui.label(RichText::new(format!("Pair with {target_label}")).strong());
                    ui.horizontal(|ui| {
                        ui.label("Address");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.address)
                                .hint_text("192.168.1.20:4243")
                                .char_limit(255)
                                .desired_width(180.),
                        );
                        ui.label("8-digit PIN");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.pin)
                                .password(true)
                                .char_limit(8)
                                .desired_width(90.),
                        );
                        let valid_pin =
                            self.pin.len() == 8 && self.pin.bytes().all(|b| b.is_ascii_digit());
                        if ui
                            .add_enabled(
                                !self.address.is_empty() && valid_pin && !self.pairing_pending,
                                egui::Button::new(if self.pairing_pending {
                                    "Checking PIN…"
                                } else {
                                    "Pair"
                                }),
                            )
                            .clicked()
                        {
                            self.user_interacted = true;
                            let pin = Zeroizing::new(self.pin.clone());
                            self.pin.zeroize();
                            let result = self.worker.command(Command::Pair {
                                address: self.address.clone(),
                                pin,
                                expected: self.selected.clone(),
                            });
                            if let Err(error) = result {
                                self.status = error.to_string();
                            } else {
                                self.pairing_pending = true;
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.user_interacted = true;
                            self.pairing_pending = false;
                            self.pair_step = PairStep::None;
                        }
                    });
                    if self.status.starts_with("Pairing failed") {
                        ui.colored_label(Color32::from_rgb(230, 100, 100), &self.status);
                    }
                });
        } else if self.pair_step == PairStep::Fingerprint {
            ui.add_space(4.);
            egui::Frame::canvas(ui.style())
                .inner_margin(8.)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(
                            "Compare the fingerprint on both screens and approve each dialog.",
                        )
                        .strong(),
                    );
                    ui.horizontal(|ui| {
                        ui.label("This device's fingerprint:");
                        ui.add(
                            egui::Label::new(
                                RichText::new(pretty_fingerprint(
                                    &self.worker.identity.fingerprint,
                                ))
                                .monospace(),
                            )
                            .selectable(true),
                        );
                    });
                });
        }

        ui.add_space(6.);
        if self.trusted.is_empty() && self.peers.is_empty() {
            ui.weak("No devices found");
        }

        for (fp, peer) in self.trusted.clone() {
            let discovered_addr = self
                .peers
                .iter()
                .find(|found| found.fingerprint == fp)
                .and_then(|found| found.addresses.first().map(ToString::to_string));
            let known_addr = discovered_addr
                .clone()
                .or_else(|| self.peer_addresses.get(&fp).cloned());
            let conn = self.connections.get(&fp).cloned();
            let is_connecting = conn.is_none() && self.connecting.contains(&fp);
            let is_selected = self.selected.as_deref() == Some(&fp);

            ui.horizontal(|ui| {
                let name_color = if conn.is_some() {
                    Color32::from_rgb(100, 220, 135)
                } else {
                    Color32::from_rgb(180, 210, 235)
                };
                if ui
                    .selectable_label(
                        is_selected,
                        RichText::new(&peer.name).color(name_color).strong(),
                    )
                    .clicked()
                {
                    self.user_interacted = true;
                    self.select(fp.clone(), known_addr.clone());
                }

                if let Some(c) = &conn {
                    let badge = match c.controller {
                        Direction::Sending => "Controlling",
                        Direction::Receiving => "Controlled",
                        Direction::Local => "Connected",
                    };
                    ui.colored_label(Color32::from_rgb(100, 200, 130), badge);
                } else if is_connecting {
                    ui.colored_label(
                        Color32::from_rgb(230, 180, 90),
                        if let Some(addr) = &discovered_addr {
                            format!("Connecting · {addr}")
                        } else {
                            "Waiting on LAN…".into()
                        },
                    );
                } else if let Some(addr) = &discovered_addr {
                    ui.weak(format!("Nearby · {addr}"));
                } else {
                    ui.weak("Offline");
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Unpair").clicked() {
                        self.user_interacted = true;
                        self.command(Command::Revoke(fp.clone()));
                        if self.selected.as_deref() == Some(&fp) {
                            self.selected = None;
                        }
                    }
                    if conn.is_some() {
                        if ui.button("Disconnect").clicked() {
                            self.user_interacted = true;
                            self.command(Command::Disconnect(fp.clone()));
                        }
                    } else if is_connecting {
                        if ui.button("Cancel").clicked() {
                            self.user_interacted = true;
                            self.command(Command::Disconnect(fp.clone()));
                        }
                    } else {
                        let connect_addr = known_addr.clone().or_else(|| {
                            (is_selected && !self.address.trim().is_empty())
                                .then(|| self.address.trim().to_owned())
                        });
                        if ui.button("Connect").clicked() {
                            self.user_interacted = true;
                            self.select(fp.clone(), connect_addr.clone());
                            if let Some(addr) = connect_addr {
                                self.command(Command::Connect {
                                    address: addr,
                                    expected: Some(fp.clone()),
                                });
                            } else {
                                self.status = format!(
                                    "Enter {}'s address below, then click Connect",
                                    peer.name
                                );
                            }
                        }
                    }
                });
            });
        }

        for peer in self.peers.clone() {
            if self.trusted.contains_key(&peer.fingerprint) {
                continue;
            }
            let addr = peer.addresses.first().map(ToString::to_string);
            let is_selected = self.selected.as_deref() == Some(&peer.fingerprint);
            ui.horizontal(|ui| {
                if ui.selectable_label(is_selected, &peer.name).clicked() {
                    self.user_interacted = true;
                    self.select(peer.fingerprint.clone(), addr.clone());
                }
                if let Some(a) = &addr {
                    ui.weak(format!("Nearby · {a}"));
                } else {
                    ui.weak("Nearby");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Pair").clicked() {
                        self.user_interacted = true;
                        self.select(peer.fingerprint.clone(), addr);
                        self.pair_step = PairStep::Pin;
                    }
                });
            });
        }

        ui.add_space(6.);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Manual address");
            ui.add(
                egui::TextEdit::singleline(&mut self.address)
                    .hint_text("192.168.1.20:4243")
                    .char_limit(255)
                    .desired_width(200.),
            );
            let trimmed_addr = self.address.trim().to_owned();
            let selected_trusted = self
                .selected
                .as_ref()
                .filter(|fp| self.trusted.contains_key(*fp))
                .cloned();
            if ui
                .add_enabled(!trimmed_addr.is_empty(), egui::Button::new("Connect"))
                .clicked()
            {
                self.user_interacted = true;
                if let Some(fp) = &selected_trusted {
                    self.peer_addresses.insert(fp.clone(), trimmed_addr.clone());
                }
                self.command(Command::Connect {
                    address: trimmed_addr.clone(),
                    expected: selected_trusted,
                });
            }
            if ui
                .add_enabled(!trimmed_addr.is_empty(), egui::Button::new("Pair with PIN"))
                .clicked()
            {
                self.user_interacted = true;
                self.pair_step = PairStep::Pin;
            }
        });
    }

    fn show_handoff(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Screen layout & handoff");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Refresh monitors").clicked() {
                    self.user_interacted = true;
                    self.command(Command::Probe);
                }
                if self.monitors.len() > 1 {
                    let label = self
                        .selected_monitor
                        .clone()
                        .unwrap_or_else(|| format!("{} (default)", self.monitors[0].id));
                    let monitors = self.monitors.clone();
                    egui::ComboBox::from_id_salt("handoff_monitor")
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            for monitor in monitors {
                                let d = monitor.display;
                                let text =
                                    format!("{} ({:.0}×{:.0})", monitor.id, d.width, d.height);
                                if ui
                                    .selectable_label(
                                        self.selected_monitor.as_deref() == Some(&monitor.id),
                                        text,
                                    )
                                    .clicked()
                                {
                                    self.user_interacted = true;
                                    self.command(Command::Monitor(monitor.id));
                                }
                            }
                        });
                } else if let Some(m) = self.monitors.first() {
                    ui.weak(format!(
                        "Monitor: {} ({:.0}×{:.0})",
                        m.id, m.display.width, m.display.height
                    ));
                }
            });
        });

        if self.trusted.is_empty() {
            return;
        }

        let target_fp = self
            .selected
            .clone()
            .filter(|fp| self.trusted.contains_key(fp))
            .or_else(|| self.connections.keys().next().cloned())
            .or_else(|| self.trusted.keys().next().cloned());

        let Some(active_fp) = target_fp else {
            return;
        };
        if self.selected.as_deref() != Some(&active_fp) {
            self.selected = Some(active_fp.clone());
            self.layout = self
                .trusted
                .get(&active_fp)
                .map(|p| p.layout)
                .unwrap_or_default();
        }
        let active_name = self
            .trusted
            .get(&active_fp)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Device".into());

        ui.horizontal(|ui| {
            ui.weak(format!("Drag {active_name}:"));
            for (side, label) in [
                (Side::Left, "Left"),
                (Side::Right, "Right"),
                (Side::Top, "Top"),
                (Side::Bottom, "Bottom"),
            ] {
                if ui
                    .selectable_label(self.layout.side == side, label)
                    .clicked()
                {
                    self.user_interacted = true;
                    self.layout.side = side;
                    self.command(Command::Layout {
                        fingerprint: active_fp.clone(),
                        layout: self.layout,
                    });
                }
            }
            if self.layout.alignment.abs() > 0.01 && ui.small_button("Center").clicked() {
                self.user_interacted = true;
                self.layout.alignment = 0.0;
                self.command(Command::Layout {
                    fingerprint: active_fp.clone(),
                    layout: self.layout,
                });
            }
        });

        self.draw_interactive_layout(ui, &active_fp);
    }

    fn draw_interactive_layout(&mut self, ui: &mut egui::Ui, active_fp: &str) {
        let desired_width = (ui.available_width() - 4.).max(320.);
        let (canvas_rect, _) =
            ui.allocate_exact_size(egui::vec2(desired_width, 210.), Sense::hover());
        let painter = ui.painter_at(canvas_rect);
        painter.rect_filled(canvas_rect, 8., Color32::from_rgb(20, 22, 27));
        painter.rect_stroke(
            canvas_rect,
            8.,
            Stroke::new(1.0_f32, Color32::from_rgb(48, 52, 62)),
            egui::StrokeKind::Inside,
        );

        let local_display = self
            .selected_monitor
            .as_deref()
            .and_then(|id| self.monitors.iter().find(|m| m.id == id))
            .or_else(|| self.monitors.first())
            .map(|m| m.display);
        let local_size = box_size(local_display);
        let local_rect = Rect::from_center_size(canvas_rect.center(), local_size);

        let mut shown_fps: Vec<String> = self.connections.keys().cloned().collect();
        if !shown_fps.contains(&active_fp.to_owned()) {
            shown_fps.push(active_fp.to_owned());
        }

        let mut updated_layout: Option<(String, Layout)> = None;

        for fp in &shown_fps {
            let Some(peer) = self.trusted.get(fp).cloned() else {
                continue;
            };
            let peer_layout = if fp == active_fp {
                self.layout
            } else {
                peer.layout
            };
            let remote_display = self.connections.get(fp).map(|c| c.remote_display);
            let peer_size = box_size(remote_display);
            let snapped_center = peer_box_center(local_rect, peer_size, peer_layout);

            let drag_center = self
                .dragging
                .as_ref()
                .filter(|(drag_fp, _)| drag_fp == fp)
                .map(|(_, pos)| *pos);
            let current_center = drag_center.unwrap_or(snapped_center);
            let peer_rect = Rect::from_center_size(current_center, peer_size);

            let id = ui.make_persistent_id(("peer_box", fp.as_str()));
            let response = ui.interact(peer_rect, id, Sense::click_and_drag());

            if response.clicked() {
                self.user_interacted = true;
                let addr = self.peer_address(fp);
                self.select(fp.clone(), addr);
            }

            if response.drag_started() {
                self.user_interacted = true;
                let addr = self.peer_address(fp);
                self.select(fp.clone(), addr);
                self.dragging = Some((fp.clone(), snapped_center));
            }

            let mut preview_layout = peer_layout;
            let mut live_center = drag_center;
            if response.dragged()
                && let Some((drag_fp, pos)) = self.dragging.as_mut()
                && drag_fp == fp
            {
                *pos += response.drag_delta();
                pos.x = pos.x.clamp(
                    canvas_rect.left() + peer_size.x * 0.5 + 4.,
                    canvas_rect.right() - peer_size.x * 0.5 - 4.,
                );
                pos.y = pos.y.clamp(
                    canvas_rect.top() + peer_size.y * 0.5 + 4.,
                    canvas_rect.bottom() - peer_size.y * 0.5 - 4.,
                );
                live_center = Some(*pos);
                preview_layout = snap_layout_from_drag(local_rect, peer_size, *pos);
                self.layout = preview_layout;
            }

            if response.drag_stopped()
                && let Some((drag_fp, pos)) = self.dragging.take()
                && &drag_fp == fp
            {
                let final_layout = snap_layout_from_drag(local_rect, peer_size, pos);
                self.layout = final_layout;
                preview_layout = final_layout;
                live_center = None;
                updated_layout = Some((fp.clone(), final_layout));
            }

            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(if response.dragged() {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                });
            }

            let draw_rect = if let Some(center) = live_center {
                let ghost_center = peer_box_center(local_rect, peer_size, preview_layout);
                let ghost_rect = Rect::from_center_size(ghost_center, peer_size);
                painter.rect_stroke(
                    ghost_rect,
                    6.,
                    Stroke::new(1.5_f32, Color32::from_rgba_unmultiplied(110, 220, 150, 140)),
                    egui::StrokeKind::Inside,
                );
                Rect::from_center_size(center, peer_size)
            } else if drag_center.is_some() {
                Rect::from_center_size(
                    peer_box_center(local_rect, peer_size, preview_layout),
                    peer_size,
                )
            } else {
                peer_rect
            };

            let is_connected = self.connections.contains_key(fp);
            let is_active = fp == active_fp;
            let fill = if is_connected {
                Color32::from_rgb(52, 118, 82)
            } else {
                Color32::from_rgb(62, 82, 102)
            };
            painter.rect_filled(draw_rect, 6., fill);
            painter.rect_stroke(
                draw_rect,
                6.,
                Stroke::new(
                    if is_active { 2.0_f32 } else { 1.0_f32 },
                    if is_active {
                        Color32::from_rgb(150, 235, 180)
                    } else {
                        Color32::from_rgb(100, 125, 145)
                    },
                ),
                egui::StrokeKind::Inside,
            );
            draw_seam(
                &painter,
                local_rect,
                peer_size,
                preview_layout,
                is_connected,
            );
            painter.text(
                draw_rect.center(),
                egui::Align2::CENTER_CENTER,
                truncate_label(&peer.name, 16),
                egui::FontId::proportional(13.),
                Color32::WHITE,
            );
        }

        painter.rect_filled(local_rect, 6., Color32::from_rgb(38, 88, 128));
        painter.rect_stroke(
            local_rect,
            6.,
            Stroke::new(1.5_f32, Color32::from_rgb(110, 175, 230)),
            egui::StrokeKind::Inside,
        );
        painter.text(
            local_rect.center(),
            egui::Align2::CENTER_CENTER,
            truncate_label(&self.name, 16),
            egui::FontId::proportional(13.),
            Color32::WHITE,
        );

        if let Some((fp, layout)) = updated_layout {
            self.command(Command::Layout {
                fingerprint: fp,
                layout,
            });
        }
    }

    fn show_details(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Details").show(ui, |ui| {
            let mut restart_requested = false;
            let mut manual_update = None;
            ui.horizontal(|ui| {
                ui.label(format!("Version v{}", env!("CARGO_PKG_VERSION")));
                match &self.updates {
                    UpdateState::Checking(_) => {
                        ui.weak("· Checking for updates…");
                    }
                    UpdateState::Updating(version, _) => {
                        ui.weak(format!("· Downloading & installing v{version}…"));
                    }
                    UpdateState::Updated(version) => {
                        ui.colored_label(
                            Color32::from_rgb(100, 220, 135),
                            format!("· Updated to v{version}"),
                        );
                        if ui.small_button("Restart now").clicked() {
                            restart_requested = true;
                        }
                    }
                    UpdateState::Available(version, err) => {
                        if ui.button(format!("Update to v{version}")).clicked() {
                            manual_update = Some(version.clone());
                        }
                        if let Some(e) = err {
                            ui.weak(format!("({e})"));
                        }
                    }
                    UpdateState::Current => {
                        ui.weak("· Up to date");
                    }
                    UpdateState::Failed(error) => {
                        ui.weak(format!("· Update check failed ({error})"));
                    }
                }
                if !matches!(
                    self.updates,
                    UpdateState::Checking(_) | UpdateState::Updating(_, _)
                ) && ui.small_button("Check now").clicked()
                {
                    self.user_interacted = true;
                    self.last_update_check = Instant::now();
                    self.updates = Self::start_update_check();
                }
            });
            if let Some(v) = manual_update {
                self.updates = Self::start_self_update(v);
            }
            if restart_requested {
                self.restart_updated(ctx);
            }

            ui.horizontal(|ui| {
                ui.label(format!(
                    "Autostart: {}",
                    if self.autostart_enabled {
                        "Enabled"
                    } else {
                        "Disabled"
                    }
                ));
                let btn = if self.autostart_enabled {
                    "Disable autostart"
                } else {
                    "Enable autostart"
                };
                if ui.small_button(btn).clicked() {
                    self.user_interacted = true;
                    let target = !self.autostart_enabled;
                    match crate::updates::set_autostart(target) {
                        Ok(()) => {
                            self.autostart_enabled = target;
                            self.command(Command::MarkAutostartPrompted);
                        }
                        Err(e) => self.status = format!("Autostart failed: {e}"),
                    }
                }
                ui.separator();
                let tray_label = if self.hide_tray {
                    "Show tray icon"
                } else {
                    "Hide tray icon"
                };
                if ui.small_button(tray_label).clicked() {
                    self.user_interacted = true;
                    self.hide_tray = !self.hide_tray;
                    if self.hide_tray {
                        self.tray = None;
                    } else {
                        self.tray_failed = false;
                    }
                    self.command(Command::SetHideTray(self.hide_tray));
                }
            });
        });
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if !self.repaint_registered {
            let repaint = ctx.clone();
            self.worker.set_repaint(move || repaint.request_repaint());
            self.repaint_registered = true;
        }
        self.ensure_tray(ctx);
        while let Ok(action) = self.tray_events.try_recv() {
            match action {
                TrayAction::Show => {
                    self.user_interacted = true;
                    Self::show_window(ctx);
                }
                TrayAction::HideTray => {
                    self.hide_tray = true;
                    self.tray = None;
                    self.command(Command::SetHideTray(true));
                    self.status =
                        "Tray icon hidden — open Ravel from your system app launcher".into();
                }
                TrayAction::Quit => self.quit(ctx),
            }
        }
        self.update_events(ctx);
        ctx.request_repaint_after(Duration::from_millis(100));
        if self.exited || self.exit_at.is_some_and(|t| Instant::now() >= t) {
            self.quitting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input(|input| input.viewport().close_requested()) && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.can_hide && (self.tray.is_some() || self.hide_tray) {
                Self::hide_window(ctx);
            } else {
                self.status = "Hiding needs X11/Xwayland on Linux".into();
            }
        }
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(8.);
            ui.horizontal(|ui| {
                ui.heading("Ravel");
                ui.weak(format!("v{}", env!("CARGO_PKG_VERSION")));
                ui.separator();
                ui.label("This device:");
                let name_edit = ui.add(
                    egui::TextEdit::singleline(&mut self.name_draft)
                        .char_limit(64)
                        .desired_width(150.),
                );
                let next_name = self.name_draft.trim().to_owned();
                let name_changed = !next_name.is_empty() && next_name != self.name;
                if ui
                    .add_enabled(name_changed, egui::Button::new("Save"))
                    .clicked()
                    || (name_changed
                        && name_edit.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    self.user_interacted = true;
                    self.command(Command::SetName(next_name));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    #[cfg(target_os = "linux")]
                    {
                        let tex = self
                            .close_icon
                            .get_or_insert_with(|| {
                                load_system_close_icon(32).map(|img| {
                                    ctx.load_texture(
                                        "system-window-close",
                                        img,
                                        egui::TextureOptions::LINEAR,
                                    )
                                })
                            })
                            .clone();
                        let (btn_rect, resp) =
                            ui.allocate_exact_size(Vec2::splat(26.), Sense::click());
                        let hovered = resp.hovered();
                        if hovered || resp.is_pointer_button_down_on() {
                            let bg = if resp.is_pointer_button_down_on() {
                                Color32::from_rgba_unmultiplied(255, 255, 255, 42)
                            } else {
                                Color32::from_rgba_unmultiplied(255, 255, 255, 24)
                            };
                            ui.painter().circle_filled(btn_rect.center(), 12.5, bg);
                        }
                        let tint = if hovered {
                            Color32::WHITE
                        } else {
                            Color32::from_rgb(220, 224, 230)
                        };
                        if let Some(tex) = tex {
                            let icon_rect =
                                Rect::from_center_size(btn_rect.center(), Vec2::splat(16.));
                            ui.painter().image(
                                tex.id(),
                                icon_rect,
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                                tint,
                            );
                        } else {
                            let c = btn_rect.center();
                            let d = 4.5;
                            let stroke = Stroke::new(1.7_f32, tint);
                            ui.painter().line_segment(
                                [c + egui::vec2(-d, -d), c + egui::vec2(d, d)],
                                stroke,
                            );
                            ui.painter().line_segment(
                                [c + egui::vec2(-d, d), c + egui::vec2(d, -d)],
                                stroke,
                            );
                        }
                        if resp
                            .on_hover_text("Minimize to tray / background")
                            .clicked()
                        {
                            if self.can_hide && (self.tray.is_some() || self.hide_tray) {
                                Self::hide_window(ctx);
                            } else {
                                self.status = "Hiding needs X11/Xwayland on Linux".into();
                            }
                        }
                    }
                    let sharing_button = if self.enabled {
                        egui::Button::new(RichText::new("Stop sharing").color(Color32::WHITE))
                            .fill(Color32::from_rgb(160, 45, 45))
                    } else {
                        egui::Button::new("Start sharing")
                    };
                    if ui.add(sharing_button).clicked() {
                        self.user_interacted = true;
                        if self.enabled {
                            self.worker.stop();
                            self.enabled = false;
                            self.offer = None;
                        } else {
                            self.command(Command::Enable);
                        }
                    }
                });
            });

            let sending_to = self
                .connections
                .values()
                .find(|c| c.controller == Direction::Sending)
                .map(|c| c.name.clone());
            let receiving_from = self
                .connections
                .values()
                .find(|c| c.controller == Direction::Receiving)
                .map(|c| c.name.clone());
            let connected_names: Vec<String> =
                self.connections.values().map(|c| c.name.clone()).collect();

            ui.horizontal(|ui| {
                if let Some(peer_name) = sending_to {
                    ui.colored_label(
                        Color32::from_rgb(100, 220, 135),
                        RichText::new(format!("Controlling {peer_name}")).strong(),
                    );
                } else if let Some(peer_name) = receiving_from {
                    ui.colored_label(
                        Color32::from_rgb(230, 180, 90),
                        RichText::new(format!("{peer_name} is controlling this device")).strong(),
                    );
                    if ui.button("Take control on this device").clicked() {
                        self.user_interacted = true;
                        self.command(Command::TakeControl);
                    }
                } else if !connected_names.is_empty() {
                    ui.colored_label(
                        Color32::from_rgb(100, 200, 130),
                        RichText::new(format!("Connected: {}", connected_names.join(", ")))
                            .strong(),
                    );
                } else if self.backend.starts_with("Backend unavailable") {
                    ui.colored_label(
                        Color32::from_rgb(230, 100, 100),
                        RichText::new(&self.backend).strong(),
                    );
                } else if self.enabled {
                    ui.colored_label(
                        Color32::from_rgb(100, 200, 130),
                        RichText::new("Ready to connect").strong(),
                    );
                } else {
                    ui.colored_label(
                        Color32::from_rgb(230, 180, 90),
                        RichText::new("Sharing is off").strong(),
                    );
                }
            });

            if let Some((progress, incoming, since)) = self.handoff
                && (progress < 1. && since.elapsed() < Duration::from_secs(2)
                    || progress >= 1. && since.elapsed() < Duration::from_millis(900))
            {
                let shown = if incoming && progress >= 1. {
                    (since.elapsed().as_secs_f32() / 0.6).min(1.)
                } else {
                    progress
                };
                ui.add(egui::ProgressBar::new(shown).text(if incoming {
                    "Control entering this device"
                } else if progress >= 1. {
                    "Control switched to the other device"
                } else {
                    "Push through the edge to switch"
                }));
                ctx.request_repaint_after(Duration::from_millis(16));
            }
            ui.add_space(6.);
        });
        egui::TopBottomPanel::bottom("bottom_actions")
            .exact_height(40.)
            .show(ctx, |ui| {
                ui.add_space(6.);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Kill switch").clicked() {
                        self.quit(ctx);
                    }
                });
                ui.add_space(6.);
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
                    self.show_handoff(ui);
                });
                ui.add_space(8.);
                self.show_details(ctx, ui);
            });
        });
        if let Some(c) = self.confirmations.first() {
            let mut answer = None;
            egui::Window::new("Verify the device fingerprint")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("The PIN exchange succeeded with {}.", c.name));
                    ui.label(
                        "Compare this full fingerprint with the one shown on the other device:",
                    );
                    ui.add(
                        egui::Label::new(
                            RichText::new(pretty_fingerprint(&c.fingerprint)).monospace(),
                        )
                        .selectable(true),
                    );
                    ui.label(format!(
                        "Approval expires in {}s",
                        c.deadline
                            .saturating_duration_since(Instant::now())
                            .as_secs()
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Fingerprints match — trust device").clicked() {
                            answer = Some(true);
                        }
                        if ui.button("Reject").clicked() {
                            answer = Some(false);
                        }
                    });
                });
            if let Some(answer) = answer {
                let c = self.confirmations.remove(0);
                let _ = c.reply.send(answer);
            }
        }
        if self.show_autostart_prompt && self.confirmations.is_empty() {
            let mut close_prompt = false;
            egui::Window::new("Launch Ravel on startup?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(
                        "Start Ravel automatically when you log in so your devices connect right away.",
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Enable autostart").clicked() {
                            match crate::updates::set_autostart(true) {
                                Ok(()) => self.autostart_enabled = true,
                                Err(e) => self.status = format!("Autostart failed: {e}"),
                            }
                            close_prompt = true;
                        }
                        if ui.button("Not now").clicked() {
                            close_prompt = true;
                        }
                    });
                });
            if close_prompt {
                self.show_autostart_prompt = false;
                self.command(Command::MarkAutostartPrompted);
            }
        }
        if self.show_update && self.confirmations.is_empty() && !self.show_autostart_prompt {
            let mut dismiss = false;
            let mut restart = false;
            let mut retry_version = None;
            match &self.updates {
                UpdateState::Updating(version, _) => {
                    let version = version.clone();
                    egui::Window::new("Updating Ravel…")
                        .collapsible(false)
                        .resizable(false)
                        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                        .show(ctx, |ui| {
                            ui.label(format!("Downloading and installing Ravel v{version}…"));
                        });
                }
                UpdateState::Updated(version) => {
                    let version = version.clone();
                    egui::Window::new("Ravel updated")
                        .collapsible(false)
                        .resizable(false)
                        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                        .show(ctx, |ui| {
                            ui.label(format!(
                                "Ravel v{version} has been installed. Restart to use the new version."
                            ));
                            ui.horizontal(|ui| {
                                if ui.button("Restart Ravel now").clicked() {
                                    restart = true;
                                }
                                if ui.button("Later").clicked() {
                                    self.dismissed_update = Some(version.clone());
                                    dismiss = true;
                                }
                            });
                        });
                }
                UpdateState::Available(version, error) => {
                    let version = version.clone();
                    let error = error.clone();
                    egui::Window::new("Ravel update available")
                        .collapsible(false)
                        .resizable(false)
                        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                        .show(ctx, |ui| {
                            ui.label(format!(
                                "Ravel v{version} is available. You have v{}.",
                                env!("CARGO_PKG_VERSION")
                            ));
                            if let Some(err) = error {
                                ui.weak(format!("Automatic install did not complete: {err}"));
                            }
                            ui.horizontal(|ui| {
                                if ui.button("Update now").clicked() {
                                    retry_version = Some(version.clone());
                                }
                                if ui.button("Open GitHub release").clicked() {
                                    ctx.open_url(egui::OpenUrl::new_tab(
                                        crate::updates::release_url(&version),
                                    ));
                                    self.dismissed_update = Some(version.clone());
                                    dismiss = true;
                                }
                                if ui.button("Later").clicked() {
                                    self.dismissed_update = Some(version.clone());
                                    dismiss = true;
                                }
                            });
                        });
                }
                _ => {
                    dismiss = true;
                }
            }
            if let Some(v) = retry_version {
                self.updates = Self::start_self_update(v);
            }
            if restart {
                self.restart_updated(ctx);
            } else if dismiss {
                self.show_update = false;
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

fn box_size(display: Option<Display>) -> Vec2 {
    if let Some(d) = display
        && d.width > 1.0
        && d.height > 1.0
    {
        let ratio = (d.width / d.height).clamp(0.6, 2.4) as f32;
        let h = 58.0;
        egui::vec2((h * ratio).round(), h)
    } else {
        egui::vec2(96., 58.)
    }
}

fn peer_box_center(local_rect: Rect, peer_size: Vec2, layout: Layout) -> Pos2 {
    let gap = 4.0;
    let align = layout.alignment as f32;
    let c = local_rect.center();
    match layout.side {
        Side::Left => Pos2::new(
            local_rect.left() - gap - peer_size.x * 0.5,
            c.y + align * local_rect.height(),
        ),
        Side::Right => Pos2::new(
            local_rect.right() + gap + peer_size.x * 0.5,
            c.y + align * local_rect.height(),
        ),
        Side::Top => Pos2::new(
            c.x + align * local_rect.width(),
            local_rect.top() - gap - peer_size.y * 0.5,
        ),
        Side::Bottom => Pos2::new(
            c.x + align * local_rect.width(),
            local_rect.bottom() + gap + peer_size.y * 0.5,
        ),
    }
}

fn snap_layout_from_drag(local_rect: Rect, peer_size: Vec2, drag_center: Pos2) -> Layout {
    let delta = drag_center - local_rect.center();
    let nx = delta.x / ((local_rect.width() + peer_size.x) * 0.5).max(1.);
    let ny = delta.y / ((local_rect.height() + peer_size.y) * 0.5).max(1.);
    let (side, mut alignment) = if nx.abs() >= ny.abs() {
        let side = if nx < 0.0 { Side::Left } else { Side::Right };
        let align = (delta.y / local_rect.height().max(1.)) as f64;
        (side, align.clamp(-0.9, 0.9))
    } else {
        let side = if ny < 0.0 { Side::Top } else { Side::Bottom };
        let align = (delta.x / local_rect.width().max(1.)) as f64;
        (side, align.clamp(-0.9, 0.9))
    };
    if alignment.abs() < 0.05 {
        alignment = 0.0;
    }
    Layout { side, alignment }
}

fn draw_seam(
    painter: &egui::Painter,
    local_rect: Rect,
    peer_size: Vec2,
    layout: Layout,
    connected: bool,
) {
    let peer_rect =
        Rect::from_center_size(peer_box_center(local_rect, peer_size, layout), peer_size);
    let color = if connected {
        Color32::from_rgb(110, 245, 160)
    } else {
        Color32::from_rgb(120, 195, 240)
    };
    let stroke = Stroke::new(3.0_f32, color);
    match layout.side {
        Side::Left | Side::Right => {
            let top = local_rect.top().max(peer_rect.top());
            let bottom = local_rect.bottom().min(peer_rect.bottom());
            if bottom > top {
                let x = if layout.side == Side::Left {
                    local_rect.left() - 2.0
                } else {
                    local_rect.right() + 2.0
                };
                painter.line_segment([Pos2::new(x, top), Pos2::new(x, bottom)], stroke);
            }
        }
        Side::Top | Side::Bottom => {
            let left = local_rect.left().max(peer_rect.left());
            let right = local_rect.right().min(peer_rect.right());
            if right > left {
                let y = if layout.side == Side::Top {
                    local_rect.top() - 2.0
                } else {
                    local_rect.bottom() + 2.0
                };
                painter.line_segment([Pos2::new(left, y), Pos2::new(right, y)], stroke);
            }
        }
    }
}

fn truncate_label(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        text.to_owned()
    } else {
        let mut s: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> Rect {
        Rect::from_center_size(Pos2::new(200., 100.), egui::vec2(100., 60.))
    }

    #[test]
    fn dragging_to_each_edge_selects_that_side() {
        let peer = egui::vec2(100., 60.);
        for (side, offset) in [
            (Side::Left, egui::vec2(-120., 0.)),
            (Side::Right, egui::vec2(120., 0.)),
            (Side::Top, egui::vec2(0., -80.)),
            (Side::Bottom, egui::vec2(0., 80.)),
        ] {
            let layout = snap_layout_from_drag(local(), peer, local().center() + offset);
            assert_eq!(layout.side, side);
            assert_eq!(layout.alignment, 0.0);
        }
    }

    #[test]
    fn drag_alignment_is_clamped_and_roundtrips_through_box_position() {
        let peer = egui::vec2(100., 60.);
        let far = snap_layout_from_drag(local(), peer, local().center() + egui::vec2(120., 500.));
        assert!(far.alignment.abs() <= 0.9);
        let layout = Layout {
            side: Side::Right,
            alignment: 0.4,
        };
        let center = peer_box_center(local(), peer, layout);
        let back = snap_layout_from_drag(local(), peer, center);
        assert_eq!(back.side, Side::Right);
        assert!((back.alignment - 0.4).abs() < 0.01);
    }

    #[test]
    fn labels_are_truncated_on_char_boundaries() {
        assert_eq!(truncate_label("short", 16), "short");
        let out = truncate_label("ŻółćŻółćŻółćŻółćŻółć", 8);
        assert_eq!(out.chars().count(), 8);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn logo_has_transparent_corners_and_opaque_center() {
        let rgba = logo_rgba(32);
        assert_eq!(rgba.len(), 32 * 32 * 4);
        assert_eq!(rgba[3], 0);
        let mid = (16 * 32 + 16) * 4;
        assert_eq!(rgba[mid + 3], 255);
    }

    #[test]
    fn box_size_follows_aspect_ratio() {
        let wide = Display {
            x: 0.,
            y: 0.,
            width: 2560.,
            height: 1080.,
            scale: 1.,
        };
        assert!(box_size(Some(wide)).x > box_size(None).x);
        assert_eq!(box_size(None), egui::vec2(96., 58.));
    }

    #[test]
    fn svg_close_icon_rasterizer_renders_papirus_adwaita_and_breeze_paths() {
        let papirus = r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16">
            <path d="M 5,4 C 4.4477,4 4,4.4477 4,5 4,5.2652 4.1055,5.5195 4.293,5.707 L 10.293,11.707 C 10.48,11.895 10.735,12 11,12 11.552,12 12,11.552 12,11 12,10.735 11.895,10.48 11.707,10.293 L 5.707,4.293 C 5.5195,4.1055 5.2652,4 5,4 Z"/>
            <path d="M 5,12 C 4.4477,12 4,11.552 4,11 4,10.735 4.1055,10.48 4.293,10.293 L 10.293,4.293 C 10.48,4.105 10.735,4 11,4 11.552,4 12,4.448 12,5 12,5.265 11.895,5.52 11.707,5.707 L 5.707,11.707 C 5.5195,11.895 5.2652,12 5,12 Z"/>
        </svg>"#;
        let adwaita = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M4 4h1.03125c.253906.011719.511719.128906.6875.3125L8 6.59375l2.3125-2.28125c.265625-.230469.445312-.304688.6875-.3125h1v1c0 .285156-.035156.550781-.25.75L9.46875 8.03125l2.25 2.25c.1875.1875.28125.453125.28125.71875v1h-1c-.265625 0-.53125-.09375-.71875-.28125L8 9.4375l-2.28125 2.28125C5.53125 11.90625 5.265625 12 5 12H4v-1c0-.265625.09375-.53125.28125-.71875l2.28125-2.25L4.28125 5.75C4.070312 5.554688 3.976562 5.28125 4 5zm0 0"/></svg>"#;
        let breeze = r#"<svg viewBox="0 0 16 16"><path d="M 4,3.1523438 3.1523438,4 7.1523437,8 3.1523438,12 4,12.847656 8,8.8476563 12,12.847656 12.847656,12 8.8476563,8 12.847656,4 12,3.1523438 8,7.1523437 Z"/></svg>"#;

        for svg in [papirus, adwaita, breeze] {
            let img = rasterize_svg_icon(svg, 32).expect("should rasterize icon");
            assert_eq!(img.size, [32, 32]);
            // Center of the X (around 16, 16) is filled; corner (0, 0) is transparent.
            assert_eq!(img.pixels[0].a(), 0);
            assert!(img.pixels[16 * 32 + 16].a() > 200);
        }
    }
}
