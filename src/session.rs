use crate::{
    geometry::{Display, Layout, Monitor, Side, monitor_bounds},
    input::{Backend, CaptureEvent, Held},
    protocol::{self, Input, Message, point_valid},
    store::Store,
};
use anyhow::{Result, anyhow, bail, ensure};
use input_event::{Event, KeyboardEvent, PointerEvent};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot, watch},
};

pub trait StreamIo: AsyncRead + AsyncWrite + Unpin {}
impl<T: AsyncRead + AsyncWrite + Unpin> StreamIo for T {}

#[derive(Clone, Debug, PartialEq)]
pub struct PeerConnection {
    pub fingerprint: String,
    pub name: String,
    pub controller: Direction,
    pub remote_display: Display,
}

pub enum HubCommand {
    Attach {
        stream: Box<dyn StreamIo>,
        fingerprint: String,
        initiator: bool,
    },
    Disconnect(String),
    UpdateLayout {
        fingerprint: String,
        layout: Layout,
    },
    Probe,
    Shutdown,
}

pub struct HubSignals {
    pub stop: watch::Receiver<u64>,
    pub stop_request: watch::Sender<u64>,
    pub take_control: watch::Receiver<u64>,
    pub store: Rc<RefCell<Store>>,
    pub status: mpsc::Sender<String>,
    pub feedback: mpsc::Sender<Feedback>,
    pub connections: mpsc::Sender<Vec<PeerConnection>>,
    pub monitors: mpsc::Sender<(Vec<Monitor>, String)>,
    pub trusted_changed: mpsc::Sender<()>,
}

pub struct Signals {
    pub stop: watch::Receiver<u64>,
    pub stop_request: watch::Sender<u64>,
    pub store: Rc<RefCell<Store>>,
    pub fingerprint: String,
    pub status: mpsc::Sender<String>,
    pub feedback: mpsc::Sender<Feedback>,
    pub take_control: watch::Receiver<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Feedback {
    Controller(Direction),
    Handoff { progress: f32, incoming: bool },
    Disconnected,
}
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub enum Direction {
    #[default]
    Local,
    Sending,
    Receiving,
}
struct HandoffGate {
    entry: [f64; 2],
    started: Instant,
    distance: f64,
    reported_step: u8,
}
impl HandoffGate {
    const DISTANCE: f64 = 96.;
    const TIMEOUT: Duration = Duration::from_millis(1500);

    fn new(entry: [f64; 2]) -> Self {
        Self {
            entry,
            started: Instant::now(),
            distance: 0.,
            reported_step: 0,
        }
    }
    fn push(&mut self, side: Side, dx: f64, dy: f64) -> f32 {
        let outward = match side {
            Side::Left => -dx,
            Side::Right => dx,
            Side::Top => -dy,
            Side::Bottom => dy,
        };
        self.distance = (self.distance + outward).clamp(0., Self::DISTANCE);
        (self.distance / Self::DISTANCE) as f32
    }
    fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.started) >= Self::TIMEOUT
    }
}
impl Direction {
    pub fn receive_enter(&mut self) -> Result<()> {
        ensure!(*self == Self::Local, "Conflicting control ownership");
        *self = Self::Receiving;
        Ok(())
    }
    pub fn receive_input(&self) -> Result<()> {
        ensure!(
            *self == Self::Receiving,
            "Input without active remote ownership"
        );
        Ok(())
    }
    pub fn receive_leave(&mut self) -> Result<()> {
        self.receive_input()?;
        *self = Self::Local;
        Ok(())
    }
}

pub async fn run<S: AsyncRead + AsyncWrite + Unpin + 'static>(
    stream: S,
    layout: Layout,
    initiator: bool,
    signals: Signals,
) -> Result<()> {
    // No OS capture/injection object exists before network authorization.
    ensure!(
        signals.store.borrow().trusted(&signals.fingerprint),
        "Peer is not trusted"
    );
    ensure!(!signals.stop.has_changed()?, "Sharing was stopped");
    let monitor = signals.store.borrow().monitor().map(str::to_owned);
    let backend = Backend::with_monitor(monitor.as_deref()).await?;
    run_with_backend(stream, layout, initiator, signals, backend).await
}

pub(crate) async fn run_with_backend<S: AsyncRead + AsyncWrite + Unpin + 'static>(
    mut stream: S,
    layout: Layout,
    initiator: bool,
    mut signals: Signals,
    mut backend: Backend,
) -> Result<()> {
    let result = async {
        ensure!(signals.store.borrow().trusted(&signals.fingerprint), "Peer trust revoked");
        ensure!(!signals.stop.has_changed()?, "Sharing was stopped");
        let exchange = async {
            backend.display.validate()?;
            backend.desktop.validate()?;
            layout.validate()?;
            protocol::write(&mut stream, &Message::Ready { display: backend.display, desktop: backend.desktop, layout }).await?;
            let Message::Ready { display: remote_display, desktop: remote_desktop, layout: remote_layout } = protocol::read(&mut stream).await? else { bail!("Expected display configuration"); };
            remote_display.validate()?;
            remote_desktop.validate()?;
            ensure!(remote_desktop.contains(remote_display), "Handoff monitor is outside the desktop");
            remote_layout.validate()?;
            let layout = if initiator { layout } else { remote_layout.reciprocal() };
            layout.validate()?;
            Ok::<_, anyhow::Error>((remote_display, remote_desktop, layout))
        };
        let Some((remote_display, remote_desktop, layout)) = (tokio::select! {
            biased;
            _ = signals.stop.changed() => None,
            result = tokio::time::timeout(Duration::from_secs(5), exchange) => Some(result.map_err(|_| anyhow!("Display exchange timed out"))??),
        }) else { return Ok(()); };
        // Trust or stop may have changed while display negotiation was pending.
        ensure!(signals.store.borrow().trusted(&signals.fingerprint), "Peer trust revoked");
        ensure!(!signals.stop.has_changed()?, "Sharing was stopped");
        signals
            .store
            .borrow_mut()
            .set_layout(&signals.fingerprint, layout)?;
        let _ = signals.status.try_send(format!(
            "Connected — {}. Move to the {:?} edge to share.",
            backend.label, layout.side
        ));
        control(stream, &mut backend, remote_display, remote_desktop, layout, &mut signals).await
    }
    .await;
    let cleanup = backend.close().await;
    result.and(cleanup)
}

struct Tasks {
    reader: tokio::task::JoinHandle<()>,
    writer: tokio::task::JoinHandle<()>,
}
impl Drop for Tasks {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}
fn send(tx: &mpsc::Sender<Message>, message: Message) -> Result<()> {
    tx.try_send(message)
        .map_err(|_| anyhow!("Session congested; sharing stopped"))
}
async fn send_stop(tx: &mpsc::Sender<oneshot::Sender<()>>) {
    let (ack, delivered) = oneshot::channel();
    if tx.try_send(ack).is_ok() {
        let _ = tokio::time::timeout(Duration::from_secs(2), delivered).await;
    }
}
async fn return_to_local(
    backend: &mut Backend,
    outgoing: &mpsc::Sender<Message>,
    direction: &mut Direction,
    held: &mut Held,
    rearm_at: &mut Option<Instant>,
    signals: &Signals,
) -> Result<()> {
    send(outgoing, Message::Leave)?;
    backend.arm(None).await?;
    let point = backend
        .desktop
        .normalize(backend.display.denormalize([0.5, 0.5]));
    backend.inject(Input::Motion {
        x: point[0],
        y: point[1],
    })?;
    *held = Held::default();
    *direction = Direction::Local;
    *rearm_at = Some(Instant::now() + Duration::from_millis(350));
    let _ = signals
        .feedback
        .try_send(Feedback::Controller(Direction::Local));
    let _ = signals.status.try_send("Connected — local control".into());
    Ok(())
}

async fn control<S: AsyncRead + AsyncWrite + Unpin + 'static>(
    stream: S,
    backend: &mut Backend,
    remote_monitor: Display,
    remote_desktop: Display,
    mut layout: Layout,
    signals: &mut Signals,
) -> Result<()> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (incoming_tx, mut incoming) = mpsc::channel::<Result<Message>>(128);
    let (outgoing, mut outgoing_rx) = mpsc::channel(128);
    let (urgent_stop, mut stop_rx) = mpsc::channel::<oneshot::Sender<()>>(1);
    let errors = incoming_tx.clone();
    let reader = tokio::task::spawn_local(async move {
        loop {
            let result =
                match tokio::time::timeout(Duration::from_secs(4), protocol::read(&mut reader))
                    .await
                {
                    Ok(result) => result,
                    Err(_) => Err(anyhow!("Peer timeout")),
                };
            let failed = result.is_err();
            if incoming_tx.send(result).await.is_err() || failed {
                break;
            }
        }
    });
    let writer = tokio::task::spawn_local(async move {
        loop {
            let (message, ack) = tokio::select! {
                biased;
                Some(ack) = stop_rx.recv() => (Message::Stop, Some(ack)),
                Some(message) = outgoing_rx.recv() => (message, None),
                else => break,
            };
            let result = match tokio::time::timeout(
                Duration::from_secs(2),
                protocol::write(&mut writer, &message),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(anyhow!("Peer write timeout")),
            };
            if let Err(error) = result {
                let _ = errors.send(Err(error)).await;
                break;
            }
            if let Some(ack) = ack {
                let _ = ack.send(());
                break;
            }
        }
    });
    let _tasks = Tasks { reader, writer };
    let mut direction = Direction::Local;
    let mut point = [0.5, 0.5];
    let mut held = Held::default();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
    let mut tick = tokio::time::interval(Duration::from_millis(25));
    let mut rearm_at = None;
    let mut handoff: Option<HandoffGate> = None;
    backend
        .arm(layout.transfer.can_send().then_some(layout.side))
        .await?;
    let _ = signals
        .feedback
        .try_send(Feedback::Controller(Direction::Local));
    loop {
        tokio::select! {
            _=signals.stop.changed()=>{send_stop(&urgent_stop).await;return Ok(());},
            _=signals.take_control.changed()=>{
                match direction {
                    Direction::Receiving => send(&outgoing, Message::ReturnControl)?,
                    Direction::Sending => {
                        return_to_local(backend,&outgoing,&mut direction,&mut held,&mut rearm_at,signals).await?;
                    },
                    Direction::Local => {},
                }
            },
            message=incoming.recv()=>{
                ensure!(signals.store.borrow().trusted(&signals.fingerprint),"Peer trust revoked");
                ensure!(!signals.stop.has_changed()?,"Sharing stopped");
                match message.ok_or_else(||anyhow!("Connection closed"))?? {
                    Message::Enter(p)=>{ensure!(layout.transfer.can_receive(),"Transfer direction does not allow remote control");ensure!(point_valid(p),"Invalid entry point");direction.receive_enter()?;handoff=None;backend.arm(None).await?;backend.inject(Input::Motion{x:p[0],y:p[1]})?;let _=signals.feedback.try_send(Feedback::Controller(Direction::Receiving));let _=signals.feedback.try_send(Feedback::Handoff{progress:1.,incoming:true});let _=signals.status.try_send("Receiving remote input — Disconnect stops sharing".into());},
                    Message::Input(event)=>{direction.receive_input()?;backend.inject(event.validate()?)?;},
                    Message::Leave=>{direction.receive_leave()?;backend.release_input()?;rearm_at=Some(Instant::now()+Duration::from_millis(350));let _=signals.feedback.try_send(Feedback::Controller(Direction::Local));let _=signals.status.try_send("Connected — local control".into());},
                    Message::ReturnControl=>{
                        ensure!(direction!=Direction::Receiving,"Conflicting control ownership");
                        if direction==Direction::Sending {return_to_local(backend,&outgoing,&mut direction,&mut held,&mut rearm_at,signals).await?;}
                    },
                    Message::Layout(next)=>{
                        next.validate()?;
                        signals.store.borrow_mut().set_layout(&signals.fingerprint, next)?;
                        layout = next;
                        handoff = None;
                        if direction == Direction::Local && rearm_at.is_none() {
                            backend.arm(layout.transfer.can_send().then_some(layout.side)).await?;
                        }
                    },
                    Message::Ping=>{},Message::Stop=>{signals.stop_request.send_modify(|e| *e=e.wrapping_add(1));return Ok(());},
                    _=>bail!("Unexpected message in control session"),
                }
            },
            event=backend.next()=>{
                let event=event?;
                match event {
                    CaptureEvent::Begin{point:entry}=>{
                        if direction!=Direction::Local || rearm_at.is_some() || !layout.transfer.can_send(){backend.release_capture().await?;continue;}
                        if !backend.display.on_edge(entry, layout.side) {backend.release_capture().await?;continue;}
                        if layout.enter(backend.display.normalize(entry)).is_some() {
                            handoff=Some(HandoffGate::new(entry));
                            let _=signals.feedback.try_send(Feedback::Handoff{progress:0.,incoming:false});
                        }else{backend.arm(None).await?;rearm_at=Some(Instant::now()+Duration::from_millis(350));}
                    },
                    CaptureEvent::Input(event)=>{
                        if handoff.as_ref().is_some_and(|gate|gate.expired(Instant::now())) {
                            handoff=None;backend.arm(None).await?;rearm_at=Some(Instant::now()+Duration::from_millis(350));
                            let _=signals.feedback.try_send(Feedback::Handoff{progress:0.,incoming:false});
                            continue;
                        }
                        if let Some(gate)=handoff.as_mut() && direction==Direction::Local {
                            if let Event::Pointer(PointerEvent::Motion{dx,dy,..})=event {
                                ensure!(dx.is_finite() && dy.is_finite() && dx.abs()<=32768. && dy.abs()<=32768.,"Invalid captured motion");
                                let progress=gate.push(layout.side,dx,dy);
                                let step=(progress*10.).round() as u8;
                                if step!=gate.reported_step {gate.reported_step=step;let _=signals.feedback.try_send(Feedback::Handoff{progress,incoming:false});}
                                if progress>=1. {
                                    let mapped=layout.enter(backend.display.normalize(gate.entry)).ok_or_else(||anyhow!("Handoff edge changed"))?;
                                    point=remote_desktop.normalize(remote_monitor.denormalize(mapped));
                                    handoff=None;direction=Direction::Sending;send(&outgoing,Message::Enter(point))?;
                                    let _=signals.feedback.try_send(Feedback::Controller(Direction::Sending));
                                    let _=signals.status.try_send("Sending input — Ctrl+Alt+Shift+Esc stops sharing".into());
                                }
                            }
                            continue;
                        }
                        if direction!=Direction::Sending {continue;}
                        if let Event::Pointer(PointerEvent::Motion{dx,dy,..})=event {
                            ensure!(dx.is_finite() && dy.is_finite() && dx.abs()<=32768. && dy.abs()<=32768.,"Invalid captured motion");
                            point[0]+=dx/(remote_desktop.width-1.);point[1]+=dy/(remote_desktop.height-1.);
                            let remote_point=remote_monitor.normalize(remote_desktop.denormalize(point));
                            if layout.returns(remote_point) && let Some(p)=layout.return_point([remote_point[0].clamp(0.,1.),remote_point[1].clamp(0.,1.)]) {
                                send(&outgoing,Message::Leave)?;backend.arm(None).await?;
                                let local=backend.desktop.normalize(backend.display.denormalize(p));
                                backend.inject(Input::Motion{x:local[0],y:local[1]})?;held=Held::default();direction=Direction::Local;rearm_at=Some(Instant::now()+Duration::from_millis(350));let _=signals.feedback.try_send(Feedback::Controller(Direction::Local));
                            }else{
                                point=[point[0].clamp(0.,1.),point[1].clamp(0.,1.)];send(&outgoing,Message::Input(Input::Motion{x:point[0],y:point[1]}))?;
                            }
                        }else if let Some(input)=convert(event) {
                            input.validate()?;held.update(input);if held.emergency_stop(){signals.stop_request.send_modify(|e| *e=e.wrapping_add(1));send_stop(&urgent_stop).await;return Ok(());}
                            send(&outgoing,Message::Input(input))?;
                        }
                    }
                }
            },
            _=heartbeat.tick()=>{backend.check_geometry()?;ensure!(signals.store.borrow().trusted(&signals.fingerprint),"Peer trust revoked");send(&outgoing,Message::Ping)?;},
            _=tick.tick()=>{if backend.maintenance()? {signals.stop_request.send_modify(|e| *e=e.wrapping_add(1));send_stop(&urgent_stop).await;return Ok(());}
                if handoff.as_ref().is_some_and(|gate|gate.expired(Instant::now())){handoff=None;backend.arm(None).await?;rearm_at=Some(Instant::now()+Duration::from_millis(350));let _=signals.feedback.try_send(Feedback::Handoff{progress:0.,incoming:false});}
                if rearm_at.is_some_and(|t|Instant::now()>=t) && direction==Direction::Local{backend.arm(layout.transfer.can_send().then_some(layout.side)).await?;rearm_at=None;}},
        }
    }
}
fn convert(event: Event) -> Option<Input> {
    Some(match event {
        Event::Keyboard(KeyboardEvent::Key { key, state, .. }) => Input::Key {
            code: key,
            down: state != 0,
        },
        Event::Keyboard(KeyboardEvent::Modifiers {
            depressed,
            latched,
            locked,
            group,
        }) => Input::Modifiers {
            depressed,
            latched,
            locked,
            group,
        },
        Event::Pointer(PointerEvent::Button { button, state, .. }) => Input::Button {
            code: button,
            down: state != 0,
        },
        Event::Pointer(PointerEvent::Axis { axis, value, .. }) => Input::Scroll {
            axis,
            value: value / 15. * 120.,
        },
        Event::Pointer(PointerEvent::AxisDiscrete120 { axis, value }) => Input::Scroll {
            axis,
            value: value as f64,
        },
        Event::Pointer(PointerEvent::Motion { .. }) => return None,
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
enum HubDirection {
    #[default]
    Local,
    Sending(String),
    Receiving(String),
}

struct HubGate {
    fingerprint: String,
    gate: HandoffGate,
}

struct PeerSession {
    remote_monitor: Display,
    remote_desktop: Display,
    layout: Layout,
    outgoing: mpsc::Sender<Message>,
    urgent_stop: mpsc::Sender<oneshot::Sender<()>>,
    _tasks: Tasks,
}

struct ReadyPeer {
    stream: Box<dyn StreamIo>,
    fingerprint: String,
    remote_display: Display,
    remote_desktop: Display,
    layout: Layout,
}

pub async fn run_hub(commands: mpsc::Receiver<HubCommand>, signals: HubSignals) -> Result<()> {
    run_hub_inner(commands, signals, None).await
}

#[cfg(test)]
pub(crate) async fn run_hub_with_backend(
    commands: mpsc::Receiver<HubCommand>,
    signals: HubSignals,
    backend: Backend,
) -> Result<()> {
    run_hub_inner(commands, signals, Some(backend)).await
}

async fn next_hub_capture(backend: &mut Option<Backend>) -> Result<(Option<Side>, CaptureEvent)> {
    match backend {
        Some(b) => b.next_with_side().await,
        None => std::future::pending().await,
    }
}

fn emit_connections(
    peers: &BTreeMap<String, PeerSession>,
    direction: &HubDirection,
    signals: &HubSignals,
) {
    let trusted = signals.store.borrow().peers();
    let list = peers
        .iter()
        .map(|(fp, peer)| {
            let name = trusted
                .get(fp)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| fp[..8].to_owned());
            let controller = match direction {
                HubDirection::Sending(to) if to == fp => Direction::Sending,
                HubDirection::Receiving(from) if from == fp => Direction::Receiving,
                _ => Direction::Local,
            };
            PeerConnection {
                fingerprint: fp.clone(),
                name,
                controller,
                remote_display: peer.remote_monitor,
            }
        })
        .collect();
    let _ = signals.connections.try_send(list);
    let summary = match direction {
        HubDirection::Local => Direction::Local,
        HubDirection::Sending(_) => Direction::Sending,
        HubDirection::Receiving(_) => Direction::Receiving,
    };
    let _ = signals.feedback.try_send(Feedback::Controller(summary));
}

async fn arm_all(
    backend: &mut Option<Backend>,
    peers: &BTreeMap<String, PeerSession>,
    signals: &HubSignals,
) -> Result<()> {
    let Some(b) = backend.as_mut() else {
        return Ok(());
    };
    if peers.is_empty() {
        return b.arm_sides(&[]).await;
    }
    let bounds = monitor_bounds(&b.monitors)?;
    let mut sides = Vec::new();
    for peer in peers.values() {
        if !peer.layout.transfer.can_send() {
            continue;
        }
        if b.display.is_outer_edge(bounds, peer.layout.side) {
            if !sides.contains(&peer.layout.side) {
                sides.push(peer.layout.side);
            }
        } else {
            let _ = signals.status.try_send(format!(
                "Selected handoff monitor is not on the outer {:?} edge",
                peer.layout.side
            ));
        }
    }
    b.arm_sides(&sides).await?;
    if std::env::var("RAVEL_DEBUG").as_deref() == Ok("1") {
        eprintln!("[ravel] capture armed: {sides:?} — {}", b.label);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn remove_peer(
    fp: &str,
    send_stop_msg: bool,
    peers: &mut BTreeMap<String, PeerSession>,
    backend: &mut Option<Backend>,
    direction: &mut HubDirection,
    held: &mut Held,
    handoff: &mut Option<HubGate>,
    rearm_at: &mut Option<Instant>,
    pending_handshakes: usize,
    signals: &HubSignals,
) {
    let Some(removed) = peers.remove(fp) else {
        return;
    };
    if send_stop_msg {
        send_stop(&removed.urgent_stop).await;
    }
    if handoff.as_ref().is_some_and(|h| h.fingerprint == fp) {
        *handoff = None;
    }
    if matches!(direction, HubDirection::Receiving(from) if from == fp) {
        if let Some(b) = backend.as_mut() {
            let _ = b.release_input();
        }
        *direction = HubDirection::Local;
        *rearm_at = Some(Instant::now() + Duration::from_millis(350));
    } else if matches!(direction, HubDirection::Sending(to) if to == fp) {
        if let Some(b) = backend.as_mut() {
            let _ = b.arm_sides(&[]).await;
            let center = b.desktop.normalize(b.display.denormalize([0.5, 0.5]));
            let _ = b.inject(Input::Motion {
                x: center[0],
                y: center[1],
            });
        }
        *held = Held::default();
        *direction = HubDirection::Local;
        *rearm_at = Some(Instant::now() + Duration::from_millis(350));
    }
    if peers.is_empty() && pending_handshakes == 0 {
        if let Some(mut b) = backend.take() {
            let _ = b.close().await;
        }
        *direction = HubDirection::Local;
        *handoff = None;
        *rearm_at = None;
        *held = Held::default();
        let _ = signals.connections.try_send(Vec::new());
        let _ = signals.feedback.try_send(Feedback::Disconnected);
        let _ = signals
            .status
            .try_send("Disconnected — held input released".into());
    } else {
        if *direction == HubDirection::Local
            && rearm_at.is_none()
            && let Err(e) = arm_all(backend, peers, signals).await
        {
            let _ = signals.status.try_send(format!("Handoff error: {e}"));
        }
        emit_connections(peers, direction, signals);
    }
}

async fn stop_all_peers(
    peers: &mut BTreeMap<String, PeerSession>,
    backend: &mut Option<Backend>,
    direction: &mut HubDirection,
    held: &mut Held,
    handoff: &mut Option<HubGate>,
    rearm_at: &mut Option<Instant>,
    signals: &HubSignals,
) {
    let taken = std::mem::take(peers);
    for (_, peer) in taken {
        send_stop(&peer.urgent_stop).await;
    }
    if let Some(mut b) = backend.take() {
        let _ = b.close().await;
    }
    *direction = HubDirection::Local;
    *held = Held::default();
    *handoff = None;
    *rearm_at = None;
    let _ = signals.connections.try_send(Vec::new());
    let _ = signals.feedback.try_send(Feedback::Disconnected);
}

async fn run_hub_inner(
    mut commands: mpsc::Receiver<HubCommand>,
    mut signals: HubSignals,
    initial_backend: Option<Backend>,
) -> Result<()> {
    let mut backend: Option<Backend> = initial_backend;
    let mut peers: BTreeMap<String, PeerSession> = BTreeMap::new();
    let mut direction = HubDirection::Local;
    let mut point = [0.5, 0.5];
    let mut held = Held::default();
    let mut handoff: Option<HubGate> = None;
    let mut rearm_at: Option<Instant> = None;
    let (incoming_tx, mut incoming_rx) = mpsc::channel::<(String, Result<Message>)>(256);
    let mut handshakes = tokio::task::JoinSet::new();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
    let mut tick = tokio::time::interval(Duration::from_millis(25));
    let mut stop_epoch = *signals.stop.borrow_and_update();

    loop {
        tokio::select! {
            biased;
            changed = signals.stop.changed() => {
                if changed.is_err() {
                    break;
                }
                stop_epoch = *signals.stop.borrow_and_update();
                handshakes.abort_all();
                stop_all_peers(&mut peers, &mut backend, &mut direction, &mut held, &mut handoff, &mut rearm_at, &signals).await;
            },
            _ = signals.take_control.changed() => {
                match &direction {
                    HubDirection::Receiving(from) => {
                        if let Some(peer) = peers.get(from) {
                            let _ = send(&peer.outgoing, Message::ReturnControl);
                        }
                    }
                    HubDirection::Sending(to) => {
                        let to = to.clone();
                        if let Some(peer) = peers.get(&to) {
                            let _ = send(&peer.outgoing, Message::Leave);
                        }
                        if let Some(b) = backend.as_mut() {
                            let _ = b.arm_sides(&[]).await;
                            let center = b.desktop.normalize(b.display.denormalize([0.5, 0.5]));
                            let _ = b.inject(Input::Motion { x: center[0], y: center[1] });
                        }
                        held = Held::default();
                        direction = HubDirection::Local;
                        rearm_at = Some(Instant::now() + Duration::from_millis(350));
                        emit_connections(&peers, &direction, &signals);
                        let _ = signals.status.try_send("Connected — local control".into());
                    }
                    HubDirection::Local => {}
                }
            },
            cmd = commands.recv() => {
                let Some(cmd) = cmd else {
                    break;
                };
                match cmd {
                    HubCommand::Shutdown => break,
                    HubCommand::Attach { mut stream, fingerprint, initiator } => {
                        if !signals.store.borrow().trusted(&fingerprint) {
                            continue;
                        }
                        if backend.is_none() {
                            let monitor = signals.store.borrow().monitor().map(str::to_owned);
                            match Backend::with_monitor(monitor.as_deref()).await {
                                Ok(b) => {
                                    let _ = signals.monitors.try_send((
                                        b.monitors.clone(),
                                        format!("{} ({} monitor{})", b.label, b.monitors.len(), if b.monitors.len() == 1 { "" } else { "s" }),
                                    ));
                                    backend = Some(b);
                                }
                                Err(e) => {
                                    let msg = format!("Backend unavailable: {e:#}");
                                    let _ = signals.monitors.try_send((Vec::new(), msg.clone()));
                                    let _ = signals.status.try_send(msg);
                                    continue;
                                }
                            }
                        }
                        let b = backend.as_ref().unwrap();
                        let display = b.display;
                        let desktop = b.desktop;
                        let layout = signals.store.borrow().layout(&fingerprint);
                        let mut stop_rx = signals.stop.clone();
                        let expected_epoch = stop_epoch;
                        handshakes.spawn_local(async move {
                            let exchange = async {
                                display.validate()?;
                                desktop.validate()?;
                                layout.validate()?;
                                protocol::write(&mut stream, &Message::Ready { display, desktop, layout }).await?;
                                let Message::Ready { display: remote_display, desktop: remote_desktop, layout: remote_layout } = protocol::read(&mut stream).await? else {
                                    bail!("Expected display configuration");
                                };
                                remote_display.validate()?;
                                remote_desktop.validate()?;
                                ensure!(remote_desktop.contains(remote_display), "Handoff monitor is outside the desktop");
                                remote_layout.validate()?;
                                let resolved = if initiator { layout } else { remote_layout.reciprocal() };
                                resolved.validate()?;
                                Ok::<_, anyhow::Error>(ReadyPeer {
                                    stream,
                                    fingerprint,
                                    remote_display,
                                    remote_desktop,
                                    layout: resolved,
                                })
                            };
                            if *stop_rx.borrow() != expected_epoch {
                                return None;
                            }
                            let res = tokio::select! {
                                biased;
                                _ = stop_rx.changed() => return None,
                                r = tokio::time::timeout(Duration::from_secs(5), exchange) => {
                                    r.map_err(|_| anyhow!("Display exchange timed out")).and_then(|v| v)
                                }
                            };
                            Some(res)
                        });
                    }
                    HubCommand::Disconnect(fp) => {
                        remove_peer(
                            &fp,
                            true,
                            &mut peers,
                            &mut backend,
                            &mut direction,
                            &mut held,
                            &mut handoff,
                            &mut rearm_at,
                            handshakes.len(),
                            &signals,
                        ).await;
                    }
                    HubCommand::UpdateLayout { fingerprint, layout } => {
                        if let Some(peer) = peers.get_mut(&fingerprint) {
                            peer.layout = layout;
                            handoff = None;
                            let _ = send(&peer.outgoing, Message::Layout(layout.reciprocal()));
                            if direction == HubDirection::Local
                                && rearm_at.is_none()
                                && let Err(e) = arm_all(&mut backend, &peers, &signals).await
                            {
                                let _ = signals.status.try_send(format!("Handoff error: {e}"));
                            }
                        }
                    }
                    HubCommand::Probe => {
                        if let Some(b) = backend.as_ref() {
                            let _ = signals.monitors.try_send((
                                b.monitors.clone(),
                                format!("{} ({} monitor{})", b.label, b.monitors.len(), if b.monitors.len() == 1 { "" } else { "s" }),
                            ));
                        } else {
                            let monitor = signals.store.borrow().monitor().map(str::to_owned);
                            let monitors_tx = signals.monitors.clone();
                            tokio::task::spawn_local(async move {
                                match Backend::with_monitor(monitor.as_deref()).await {
                                    Ok(mut b) => {
                                        let list = b.monitors.clone();
                                        let text = match b.close().await {
                                            Ok(()) => format!("{} ({} monitor{})", b.label, list.len(), if list.len() == 1 { "" } else { "s" }),
                                            Err(e) => format!("Backend cleanup failed: {e}"),
                                        };
                                        let _ = monitors_tx.try_send((list, text));
                                    }
                                    Err(e) => {
                                        let _ = monitors_tx.try_send((Vec::new(), format!("Backend unavailable: {e:#}")));
                                    }
                                }
                            });
                        }
                    }
                }
            },
            Some(completed) = handshakes.join_next() => {
                // Install the handshake result before deciding whether capture is unused.
                let ready_res = match completed {
                    Ok(Some(result)) => result,
                    Ok(None) => continue,
                    Err(error) => Err(anyhow!("Display handshake failed: {error}")),
                };
                match ready_res {
                    Ok(ready) => {
                        if backend.is_none() || !signals.store.borrow().trusted(&ready.fingerprint) {
                            if peers.is_empty() && handshakes.is_empty() && let Some(mut b) = backend.take() {
                                let _ = b.close().await;
                            }
                            continue;
                        }
                        if let Err(e) = signals.store.borrow_mut().set_layout(&ready.fingerprint, ready.layout) {
                            let _ = signals.status.try_send(format!("Could not save layout: {e}"));
                        } else {
                            let _ = signals.trusted_changed.try_send(());
                        }
                        if peers.contains_key(&ready.fingerprint) {
                            remove_peer(
                                &ready.fingerprint,
                                false,
                                &mut peers,
                                &mut backend,
                                &mut direction,
                                &mut held,
                                &mut handoff,
                                &mut rearm_at,
                                handshakes.len() + 1,
                                &signals,
                            ).await;
                        }
                        let (mut reader, mut writer) = tokio::io::split(ready.stream);
                        let (outgoing, mut outgoing_rx) = mpsc::channel(128);
                        let (urgent_stop, mut stop_rx) = mpsc::channel::<oneshot::Sender<()>>(1);
                        let fp_read = ready.fingerprint.clone();
                        let incoming_read = incoming_tx.clone();
                        let reader_task = tokio::task::spawn_local(async move {
                            loop {
                                let result = match tokio::time::timeout(Duration::from_secs(4), protocol::read(&mut reader)).await {
                                    Ok(r) => r,
                                    Err(_) => Err(anyhow!("Peer timeout")),
                                };
                                let failed = result.is_err();
                                if incoming_read.send((fp_read.clone(), result)).await.is_err() || failed {
                                    break;
                                }
                            }
                        });
                        let fp_write = ready.fingerprint.clone();
                        let incoming_err = incoming_tx.clone();
                        let writer_task = tokio::task::spawn_local(async move {
                            loop {
                                let (message, ack) = tokio::select! {
                                    biased;
                                    Some(ack) = stop_rx.recv() => (Message::Stop, Some(ack)),
                                    Some(message) = outgoing_rx.recv() => (message, None),
                                    else => break,
                                };
                                let result = match tokio::time::timeout(Duration::from_secs(2), protocol::write(&mut writer, &message)).await {
                                    Ok(r) => r,
                                    Err(_) => Err(anyhow!("Peer write timeout")),
                                };
                                if let Err(error) = result {
                                    let _ = incoming_err.send((fp_write.clone(), Err(error))).await;
                                    break;
                                }
                                if let Some(ack) = ack {
                                    let _ = ack.send(());
                                    break;
                                }
                            }
                        });
                        let peer_name = signals
                            .store
                            .borrow()
                            .peers()
                            .get(&ready.fingerprint)
                            .map(|p| p.name.clone())
                            .unwrap_or_else(|| "peer".into());
                        let label = backend.as_ref().map(|b| b.label).unwrap_or("input");
                        peers.insert(
                            ready.fingerprint.clone(),
                            PeerSession {
                                remote_monitor: ready.remote_display,
                                remote_desktop: ready.remote_desktop,
                                layout: ready.layout,
                                outgoing,
                                urgent_stop,
                                _tasks: Tasks { reader: reader_task, writer: writer_task },
                            },
                        );
                        if direction == HubDirection::Local
                            && rearm_at.is_none()
                            && let Err(e) = arm_all(&mut backend, &peers, &signals).await
                        {
                            let _ = signals.status.try_send(format!("Handoff error: {e}"));
                        }
                        emit_connections(&peers, &direction, &signals);
                        let edge_ready = backend.as_ref().is_some_and(|b| {
                            monitor_bounds(&b.monitors)
                                .is_ok_and(|bounds| b.display.is_outer_edge(bounds, ready.layout.side))
                        });
                        let status = if !ready.layout.transfer.can_send() {
                            format!("Connected to {peer_name} — receiving input only. {peer_name} can transfer control to this PC.")
                        } else if edge_ready {
                            format!(
                                "Connected to {peer_name} — {label}. Move to the {:?} edge to share.",
                                ready.layout.side
                            )
                        } else {
                            format!(
                                "Connected to {peer_name}, but handoff is disabled: select a monitor on the outer {:?} edge, then reconnect.",
                                ready.layout.side
                            )
                        };
                        let _ = signals.status.try_send(status);
                    }
                    Err(e) => {
                        let _ = signals.status.try_send(format!("Connection setup failed: {e}"));
                        if peers.is_empty() && handshakes.is_empty() && let Some(mut b) = backend.take() {
                            let _ = b.close().await;
                        }
                    }
                }
            },
            Some((fp, msg_res)) = incoming_rx.recv() => {
                if !peers.contains_key(&fp) {
                    continue;
                }
                if !signals.store.borrow().trusted(&fp) {
                    remove_peer(&fp, false, &mut peers, &mut backend, &mut direction, &mut held, &mut handoff, &mut rearm_at, handshakes.len(), &signals).await;
                    continue;
                }
                let msg = match msg_res {
                    Ok(m) => m,
                    Err(e) => {
                        remove_peer(&fp, false, &mut peers, &mut backend, &mut direction, &mut held, &mut handoff, &mut rearm_at, handshakes.len(), &signals).await;
                        let _ = signals.status.try_send(format!("Disconnected: {e}"));
                        continue;
                    }
                };
                let step_res: Result<()> = async {
                    match msg {
                        Message::Enter(p) => {
                            ensure!(peers[&fp].layout.transfer.can_receive(), "Transfer direction does not allow remote control");
                            ensure!(point_valid(p), "Invalid entry point");
                            ensure!(direction == HubDirection::Local, "Conflicting control ownership");
                            handoff = None;
                            if let Some(b) = backend.as_mut() {
                                b.arm_sides(&[]).await?;
                                b.inject(Input::Motion { x: p[0], y: p[1] })?;
                            }
                            direction = HubDirection::Receiving(fp.clone());
                            emit_connections(&peers, &direction, &signals);
                            let _ = signals.feedback.try_send(Feedback::Handoff { progress: 1., incoming: true });
                            let _ = signals.status.try_send("Receiving remote input — Disconnect stops sharing".into());
                        }
                        Message::Input(event) => {
                            ensure!(matches!(&direction, HubDirection::Receiving(from) if from == &fp), "Input without active remote ownership");
                            if let Some(b) = backend.as_mut() {
                                b.inject(event.validate()?)?;
                            }
                        }
                        Message::Leave => {
                            ensure!(matches!(&direction, HubDirection::Receiving(from) if from == &fp), "Leave without active remote ownership");
                            if let Some(b) = backend.as_mut() {
                                b.release_input()?;
                            }
                            direction = HubDirection::Local;
                            rearm_at = Some(Instant::now() + Duration::from_millis(350));
                            emit_connections(&peers, &direction, &signals);
                            let _ = signals.status.try_send("Connected — local control".into());
                        }
                        Message::ReturnControl => {
                            ensure!(!matches!(&direction, HubDirection::Receiving(from) if from == &fp), "Conflicting control ownership");
                            if matches!(&direction, HubDirection::Sending(to) if to == &fp) {
                                if let Some(peer) = peers.get(&fp) {
                                    send(&peer.outgoing, Message::Leave)?;
                                }
                                if let Some(b) = backend.as_mut() {
                                    b.arm_sides(&[]).await?;
                                    let center = b.desktop.normalize(b.display.denormalize([0.5, 0.5]));
                                    b.inject(Input::Motion { x: center[0], y: center[1] })?;
                                }
                                held = Held::default();
                                direction = HubDirection::Local;
                                rearm_at = Some(Instant::now() + Duration::from_millis(350));
                                emit_connections(&peers, &direction, &signals);
                                let _ = signals.status.try_send("Connected — local control".into());
                            }
                        }
                        Message::Layout(next_layout) => {
                            next_layout.validate()?;
                            signals.store.borrow_mut().set_layout(&fp, next_layout)?;
                            let _ = signals.trusted_changed.try_send(());
                            if let Some(peer) = peers.get_mut(&fp) {
                                peer.layout = next_layout;
                                handoff = None;
                            }
                            if direction == HubDirection::Local && rearm_at.is_none() {
                                arm_all(&mut backend, &peers, &signals).await?;
                            }
                        }
                        Message::Ping => {}
                        Message::Stop => {
                            remove_peer(&fp, false, &mut peers, &mut backend, &mut direction, &mut held, &mut handoff, &mut rearm_at, handshakes.len(), &signals).await;
                        }
                        _ => bail!("Unexpected message in control session"),
                    }
                    Ok(())
                }.await;
                if let Err(e) = step_res {
                    remove_peer(&fp, false, &mut peers, &mut backend, &mut direction, &mut held, &mut handoff, &mut rearm_at, handshakes.len(), &signals).await;
                    let _ = signals.status.try_send(format!("Session error: {e}"));
                }
            },
            capture_res = next_hub_capture(&mut backend) => {
                let (side_hint, event) = match capture_res {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = signals.status.try_send(format!("Input capture error: {e}"));
                        stop_all_peers(&mut peers, &mut backend, &mut direction, &mut held, &mut handoff, &mut rearm_at, &signals).await;
                        continue;
                    }
                };
                let Some(b) = backend.as_mut() else {
                    continue;
                };
                match event {
                    CaptureEvent::Begin { point: entry } => {
                        if direction != HubDirection::Local || rearm_at.is_some() {
                            let _ = b.release_capture().await;
                            continue;
                        }
                        let norm = b.display.normalize(entry);
                        let matched = peers.iter().find(|(_, p)| {
                            p.layout.transfer.can_send()
                                && side_hint.is_none_or(|s| s == p.layout.side)
                                && b.display.on_edge(entry, p.layout.side)
                                && p.layout.enter(norm).is_some()
                        });
                        if let Some((fp, _)) = matched {
                            handoff = Some(HubGate {
                                fingerprint: fp.clone(),
                                gate: HandoffGate::new(entry),
                            });
                            let _ = signals.feedback.try_send(Feedback::Handoff { progress: 0., incoming: false });
                        } else {
                            let _ = b.arm_sides(&[]).await;
                            rearm_at = Some(Instant::now() + Duration::from_millis(350));
                        }
                    }
                    CaptureEvent::Input(event) => {
                        if handoff.as_ref().is_some_and(|h| h.gate.expired(Instant::now())) {
                            handoff = None;
                            let _ = b.arm_sides(&[]).await;
                            rearm_at = Some(Instant::now() + Duration::from_millis(350));
                            let _ = signals.feedback.try_send(Feedback::Handoff { progress: 0., incoming: false });
                            continue;
                        }
                        if let Some(h) = handoff.as_mut() && direction == HubDirection::Local {
                            let Some(peer) = peers.get(&h.fingerprint) else {
                                handoff = None;
                                let _ = b.arm_sides(&[]).await;
                                rearm_at = Some(Instant::now() + Duration::from_millis(350));
                                continue;
                            };
                            if let Event::Pointer(PointerEvent::Motion { dx, dy, .. }) = event {
                                if !dx.is_finite() || !dy.is_finite() || dx.abs() > 32768. || dy.abs() > 32768. {
                                    continue;
                                }
                                let progress = h.gate.push(peer.layout.side, dx, dy);
                                let step = (progress * 10.).round() as u8;
                                if step != h.gate.reported_step {
                                    h.gate.reported_step = step;
                                    let _ = signals.feedback.try_send(Feedback::Handoff { progress, incoming: false });
                                }
                                if progress >= 1.
                                    && let Some(mapped) =
                                        peer.layout.enter(b.display.normalize(h.gate.entry))
                                {
                                    point = peer
                                        .remote_desktop
                                        .normalize(peer.remote_monitor.denormalize(mapped));
                                    let target_fp = h.fingerprint.clone();
                                    handoff = None;
                                    if send(&peer.outgoing, Message::Enter(point)).is_ok() {
                                        direction = HubDirection::Sending(target_fp);
                                        emit_connections(&peers, &direction, &signals);
                                        let _ = signals.status.try_send(
                                            "Sending input — Ctrl+Alt+Shift+Esc stops sharing"
                                                .into(),
                                        );
                                    } else {
                                        remove_peer(
                                            &target_fp,
                                            false,
                                            &mut peers,
                                            &mut backend,
                                            &mut direction,
                                            &mut held,
                                            &mut handoff,
                                            &mut rearm_at,
                                            handshakes.len(),
                                            &signals,
                                        )
                                        .await;
                                    }
                                }
                            }
                            continue;
                        }
                        let HubDirection::Sending(to_fp) = direction.clone() else {
                            continue;
                        };
                        let Some(peer) = peers.get(&to_fp) else {
                            continue;
                        };
                        if let Event::Pointer(PointerEvent::Motion { dx, dy, .. }) = event {
                            if !dx.is_finite() || !dy.is_finite() || dx.abs() > 32768. || dy.abs() > 32768. {
                                continue;
                            }
                            point[0] += dx / (peer.remote_desktop.width - 1.);
                            point[1] += dy / (peer.remote_desktop.height - 1.);
                            let remote_point = peer.remote_monitor.normalize(peer.remote_desktop.denormalize(point));
                            if peer.layout.returns(remote_point)
                                && let Some(p) = peer.layout.return_point([remote_point[0].clamp(0., 1.), remote_point[1].clamp(0., 1.)])
                            {
                                let _ = send(&peer.outgoing, Message::Leave);
                                let _ = b.arm_sides(&[]).await;
                                let local = b.desktop.normalize(b.display.denormalize(p));
                                let _ = b.inject(Input::Motion { x: local[0], y: local[1] });
                                held = Held::default();
                                direction = HubDirection::Local;
                                rearm_at = Some(Instant::now() + Duration::from_millis(350));
                                emit_connections(&peers, &direction, &signals);
                                let _ = signals.status.try_send("Connected — local control".into());
                            } else {
                                point = [point[0].clamp(0., 1.), point[1].clamp(0., 1.)];
                                if send(&peer.outgoing, Message::Input(Input::Motion { x: point[0], y: point[1] })).is_err() {
                                    remove_peer(&to_fp, false, &mut peers, &mut backend, &mut direction, &mut held, &mut handoff, &mut rearm_at, handshakes.len(), &signals).await;
                                }
                            }
                        } else if let Some(input) = convert(event) {
                            if input.validate().is_err() {
                                continue;
                            }
                            held.update(input);
                            if held.emergency_stop() {
                                signals.stop_request.send_modify(|e| *e = e.wrapping_add(1));
                                stop_all_peers(&mut peers, &mut backend, &mut direction, &mut held, &mut handoff, &mut rearm_at, &signals).await;
                                continue;
                            }
                            if send(&peer.outgoing, Message::Input(input)).is_err() {
                                remove_peer(&to_fp, false, &mut peers, &mut backend, &mut direction, &mut held, &mut handoff, &mut rearm_at, handshakes.len(), &signals).await;
                            }
                        }
                    }
                }
            },
            _ = heartbeat.tick() => {
                if peers.is_empty() {
                    continue;
                }
                if let Some(b) = backend.as_ref()
                    && let Err(e) = b.check_geometry()
                {
                    let _ = signals.status.try_send(e.to_string());
                    stop_all_peers(&mut peers, &mut backend, &mut direction, &mut held, &mut handoff, &mut rearm_at, &signals).await;
                    continue;
                }
                let fps: Vec<String> = peers.keys().cloned().collect();
                for fp in fps {
                    if !signals.store.borrow().trusted(&fp) {
                        remove_peer(&fp, false, &mut peers, &mut backend, &mut direction, &mut held, &mut handoff, &mut rearm_at, handshakes.len(), &signals).await;
                        continue;
                    }
                    if let Some(peer) = peers.get(&fp)
                        && send(&peer.outgoing, Message::Ping).is_err()
                    {
                        remove_peer(&fp, false, &mut peers, &mut backend, &mut direction, &mut held, &mut handoff, &mut rearm_at, handshakes.len(), &signals).await;
                    }
                }
            },
            _ = tick.tick() => {
                if let Some(b) = backend.as_mut() {
                    match b.maintenance() {
                        Ok(true) => {
                            signals.stop_request.send_modify(|e| *e = e.wrapping_add(1));
                            stop_all_peers(&mut peers, &mut backend, &mut direction, &mut held, &mut handoff, &mut rearm_at, &signals).await;
                            continue;
                        }
                        Err(e) => {
                            let _ = signals.status.try_send(format!("Input error: {e}"));
                            stop_all_peers(&mut peers, &mut backend, &mut direction, &mut held, &mut handoff, &mut rearm_at, &signals).await;
                            continue;
                        }
                        Ok(false) => {}
                    }
                    if handoff.as_ref().is_some_and(|h| h.gate.expired(Instant::now())) {
                        handoff = None;
                        let _ = b.arm_sides(&[]).await;
                        rearm_at = Some(Instant::now() + Duration::from_millis(350));
                        let _ = signals.feedback.try_send(Feedback::Handoff { progress: 0., incoming: false });
                    }
                }
                if rearm_at.is_some_and(|t| Instant::now() >= t) {
                    rearm_at = None;
                    if direction == HubDirection::Local
                        && let Err(e) = arm_all(&mut backend, &peers, &signals).await
                    {
                        let _ = signals.status.try_send(format!("Handoff error: {e}"));
                    }
                }
            }
        }
    }
    stop_all_peers(
        &mut peers,
        &mut backend,
        &mut direction,
        &mut held,
        &mut handoff,
        &mut rearm_at,
        &signals,
    )
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handoff_requires_outward_motion_and_resets_on_inward_motion() {
        for (side, dx, dy) in [
            (Side::Left, -1., 0.),
            (Side::Right, 1., 0.),
            (Side::Top, 0., -1.),
            (Side::Bottom, 0., 1.),
        ] {
            let mut gate = HandoffGate::new([0., 0.]);
            assert!(gate.push(side, dx * 20., dy * 20.) < 1.);
            assert_eq!(gate.push(side, -dx * 30., -dy * 30.), 0.);
            assert!(gate.push(side, dx * 48., dy * 48.) < 1.);
            assert_eq!(gate.push(side, dx * 48., dy * 48.), 1.);
            assert!(!gate.expired(gate.started + HandoffGate::TIMEOUT - Duration::from_millis(1)));
            assert!(gate.expired(gate.started + HandoffGate::TIMEOUT));
        }
    }
    #[test]
    fn no_input_before_ownership_and_no_overlapping_streams() {
        let mut s = Direction::Local;
        assert!(s.receive_input().is_err());
        assert!(s.receive_leave().is_err());
        s.receive_enter().unwrap();
        assert!(s.receive_enter().is_err());
        s.receive_input().unwrap();
        s.receive_leave().unwrap();
        assert!(s.receive_input().is_err());
        s = Direction::Sending;
        assert!(s.receive_enter().is_err());
    }
}

#[cfg(test)]
mod lifecycle_tests;
