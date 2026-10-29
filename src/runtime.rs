use crate::{
    discovery::{self, Discovery, Peer},
    geometry::Layout,
    identity::Identity,
    pairing::{self, Binding, Offer},
    protocol::{self, Message, VERSION},
    session::{self, Signals},
    store::{Store, TrustedPeer, validate_name},
    tls,
};
use anyhow::{Result, anyhow, bail, ensure};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Semaphore, mpsc, oneshot, watch},
    task::JoinSet,
};
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};
use zeroize::Zeroizing;

pub enum Command {
    OfferPin,
    CancelPin,
    Enable,
    Probe,
    Pair {
        address: String,
        pin: Zeroizing<String>,
        expected: Option<String>,
    },
    Connect {
        address: String,
        expected: Option<String>,
    },
    Revoke(String),
    Layout {
        fingerprint: String,
        layout: Layout,
    },
}
pub enum Event {
    Status(String),
    Backend(String),
    Peers(Vec<Peer>),
    Trusted(BTreeMap<String, TrustedPeer>),
    Pin {
        value: Zeroizing<String>,
        deadline: Instant,
    },
    PinClosed,
    Confirm {
        fingerprint: String,
        name: String,
        deadline: Instant,
        reply: oneshot::Sender<bool>,
    },
    Listening(u16),
    Enabled(bool),
}
pub struct Worker {
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
    pub identity: Arc<Identity>,
    stop: watch::Sender<u64>,
    quit: watch::Sender<bool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Worker {
    pub fn start(directory: PathBuf, name: String, port: u16) -> Result<Self> {
        validate_name(&name)?;
        let identity = Arc::new(Identity::load_or_create(&directory)?);
        let store = Store::load(&directory)?;
        let (commands, rx) = mpsc::channel(32);
        let (tx, events) = mpsc::channel(128);
        let (stop, _) = watch::channel(0);
        let (quit, quit_rx) = watch::channel(false);
        let identity_thread = identity.clone();
        let stop_thread = stop.clone();
        let thread = std::thread::Builder::new()
            .name("ravel-network".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => {
                        let local = tokio::task::LocalSet::new();
                        let event_tx = tx.clone();
                        if let Err(e) = runtime.block_on(local.run_until(work(
                            directory,
                            name,
                            port,
                            identity_thread,
                            store,
                            rx,
                            tx,
                            stop_thread,
                            quit_rx,
                        ))) {
                            let _ =
                                event_tx.try_send(Event::Status(format!("Service stopped: {e}")));
                        }
                    }
                    Err(e) => {
                        let _ =
                            tx.try_send(Event::Status(format!("Cannot start network worker: {e}")));
                    }
                }
            })?;
        Ok(Self {
            commands,
            events,
            identity,
            stop,
            quit,
            thread: Some(thread),
        })
    }
    pub fn stop(&self) {
        self.stop
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }
    pub fn command(&self, command: Command) -> Result<()> {
        self.commands
            .try_send(command)
            .map_err(|_| anyhow!("Application busy; try again"))
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
        let _ = self.quit.send(true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
struct Context {
    identity: Arc<Identity>,
    store: Rc<RefCell<Store>>,
    offer: RefCell<Option<Offer>>,
    events: mpsc::Sender<Event>,
    name: String,
    stop: watch::Sender<u64>,
    enabled: Cell<bool>,
    session: Arc<Semaphore>,
    handshakes: Arc<Semaphore>,
}
impl Context {
    fn status(&self, message: impl Into<String>) {
        let _ = self.events.try_send(Event::Status(message.into()));
    }
    fn trusted_update(&self) {
        let _ = self
            .events
            .try_send(Event::Trusted(self.store.borrow().peers()));
    }
    fn disable(&self) {
        self.enabled.set(false);
        self.offer.borrow_mut().take();
        let _ = self.events.try_send(Event::Enabled(false));
        let _ = self.events.try_send(Event::PinClosed);
    }
}
#[derive(Clone)]
struct Target {
    address: String,
    expected: Option<String>,
}

#[allow(clippy::too_many_arguments)]
async fn work(
    directory: PathBuf,
    name: String,
    port: u16,
    identity: Arc<Identity>,
    store: Store,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
    stop: watch::Sender<u64>,
    mut quit: watch::Receiver<bool>,
) -> Result<()> {
    let ctx = Rc::new(Context {
        identity,
        store: Rc::new(RefCell::new(store)),
        offer: RefCell::new(None),
        events,
        name,
        stop,
        enabled: Cell::new(false),
        session: Arc::new(Semaphore::new(1)),
        handshakes: Arc::new(Semaphore::new(4)),
    });
    let listener = TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).await?;
    let port = listener.local_addr()?.port();
    let server = tls::server(&ctx.identity)?;
    ctx.trusted_update();
    let _ = ctx.events.try_send(Event::Listening(port));
    ctx.status("Sharing stopped. Pair devices, then allow sharing on the receiving device.");
    let (discovery_tx, mut discovery_rx) = mpsc::channel(8);
    let mut discovery_task = None;
    match Discovery::new(&ctx.name, &ctx.identity.fingerprint, port) {
        Ok(discovery) => {
            let events = ctx.events.clone();
            let fp = ctx.identity.fingerprint.clone();
            discovery_task = Some(tokio::task::spawn_local(async move {
                if let Err(e) = discovery.run(fp, discovery_tx).await {
                    let _ = events.try_send(Event::Status(format!(
                        "Discovery unavailable; manual address still works: {e}"
                    )));
                }
            }));
        }
        Err(e) => ctx.status(format!(
            "Discovery unavailable; manual address still works: {e}"
        )),
    }
    let mut tasks = JoinSet::new();
    let mut peers: Vec<Peer> = vec![];
    let mut desired: Option<Target> = None;
    let outbound = Rc::new(Cell::new(false));
    let mut stopped = ctx.stop.subscribe();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut reconnect = tokio::time::interval(Duration::from_secs(3));
    loop {
        tokio::select! {
            biased;
            _=quit.changed()=>break,
            _=stopped.changed()=>{ctx.disable();desired=None;ctx.status("Sharing stopped — all control connections released");},
            Some(result)=tasks.join_next()=>{if let Err(e)=result{ctx.status(format!("Connection task failed: {e}"));}},
            Some(command)=commands.recv()=>{
                let result:Result<()>=match command {
                    Command::OfferPin=>{let offer=Offer::new(Instant::now());let _=ctx.events.try_send(Event::Pin{value:Zeroizing::new(offer.display_pin()),deadline:offer.deadline});*ctx.offer.borrow_mut()=Some(offer);Ok(())},
                    Command::CancelPin=>{ctx.offer.borrow_mut().take();let _=ctx.events.try_send(Event::PinClosed);Ok(())},
                    Command::Enable=>{ctx.enabled.set(true);let _=ctx.events.try_send(Event::Enabled(true));ctx.status("Sharing allowed for trusted devices");Ok(())},
                    Command::Probe=>{let events=ctx.events.clone();tasks.spawn_local(async move{let message=match crate::input::Backend::new().await{Ok(mut b)=>{let text=format!("{} — {:.0} × {:.0} logical units; constructor check only",b.label,b.display.width,b.display.height);match b.close().await{Ok(())=>text,Err(e)=>format!("Backend cleanup failed: {e}")}},Err(e)=>format!("Backend unavailable: {e:#}")};let _=events.try_send(Event::Backend(message));});Ok(())},
                    Command::Pair{address,pin,expected}=>{let context=ctx.clone();let flag=outbound.clone();if flag.replace(true){Err(anyhow!("An outgoing connection is already in progress"))}else{tasks.spawn_local(async move{let result=outgoing(context.clone(),Target{address,expected},Some(pin)).await;flag.set(false);if let Err(e)=result{context.status(format!("Pairing failed: {e}"));}});Ok(())}},
                    Command::Connect{address,expected}=>{ctx.enabled.set(true);let _=ctx.events.try_send(Event::Enabled(true));desired=Some(Target{address,expected});reconnect.reset_immediately();Ok(())},
                    Command::Revoke(fp)=>{ctx.stop.send_modify(|e|*e=e.wrapping_add(1));let result=ctx.store.borrow_mut().revoke(&fp);ctx.trusted_update();result},
                    Command::Layout{fingerprint,layout}=>{let result=ctx.store.borrow_mut().set_layout(&fingerprint,layout);ctx.trusted_update();result},
                };if let Err(e)=result{ctx.status(e.to_string());}
            },
            Some(found)=discovery_rx.recv()=>{peers=found;let _=ctx.events.try_send(Event::Peers(peers.clone()));},
            accepted=listener.accept()=>{
                let (stream,address)=accepted?;
                // Reject public-interface sockets even if the source address is private.
                if !discovery::lan_ip(address.ip()) || !discovery::lan_ip(stream.local_addr()?.ip()){continue;}
                let Ok(permit)=ctx.handshakes.clone().try_acquire_owned() else{continue;};
                let context=ctx.clone();let server=server.clone();
                tasks.spawn_local(async move{let _permit=permit;let result=incoming(context.clone(),stream,server).await;if let Err(e)=result{context.status(format!("Incoming connection ended: {e}"));}});
            },
            _=tick.tick()=>{
                if directory.join("stop.request").try_exists()?{let _=std::fs::remove_file(directory.join("stop.request"));ctx.stop.send_modify(|e|*e=e.wrapping_add(1));}
                if ctx.offer.borrow().as_ref().is_some_and(|o|Instant::now()>=o.deadline){ctx.offer.borrow_mut().take();let _=ctx.events.try_send(Event::PinClosed);}
            },
            _=reconnect.tick()=>{
                if ctx.enabled.get() && ctx.session.available_permits()>0 && !outbound.get() {
                    if let Some(mut target)=desired.clone(){
                        if let Some(fp)=&target.expected {
                            if !ctx.store.borrow().trusted(fp){desired=None;continue;}
                            if let Some(peer)=peers.iter().find(|p|&p.fingerprint==fp){if let Some(address)=peer.addresses.first(){target.address=address.to_string();}}
                        }
                        outbound.set(true);let context=ctx.clone();let flag=outbound.clone();tasks.spawn_local(async move{let result=outgoing(context.clone(),target,None).await;flag.set(false);if let Err(e)=result{context.status(format!("Disconnected; retrying while sharing is allowed: {e}"));}});
                    }
                }
            }
        }
    }
    ctx.disable();
    ctx.stop.send_modify(|e| *e = e.wrapping_add(1));
    if let Some(task) = discovery_task {
        task.abort();
    }
    // Sessions handle stop themselves so releases execute before tasks are dropped.
    let _ = tokio::time::timeout(Duration::from_secs(6), async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    Ok(())
}

async fn connect(address: &str) -> Result<TcpStream> {
    let addresses = discovery::resolve(address).await?;
    for address in addresses.into_iter().take(8) {
        if let Ok(Ok(stream)) =
            tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(address)).await
        {
            stream.set_nodelay(true)?;
            return Ok(stream);
        }
    }
    bail!("Cannot reach LAN peer")
}
async fn outgoing(ctx: Rc<Context>, target: Target, pin: Option<Zeroizing<String>>) -> Result<()> {
    let mut stop = ctx.stop.subscribe();
    let pairing = pin.is_some();
    let auth = async {
        let stream = connect(&target.address).await?;
        let config = tls::client(&ctx.identity, if pairing { None } else { target.expected })?;
        let stream = TlsConnector::from(config)
            .connect(
                rustls::pki_types::ServerName::try_from("ravel.local")?,
                stream,
            )
            .await?;
        authorize(ctx.clone(), TlsStream::Client(stream), true, pin).await
    };
    let result = tokio::select! {_=stop.changed()=>return Ok(()),r=tokio::time::timeout(Duration::from_secs(if pairing{310}else{15}),auth)=>r??};
    if let Some((stream, fp)) = result {
        control(ctx, stream, fp, true, stop).await?;
    }
    Ok(())
}
async fn incoming(
    ctx: Rc<Context>,
    stream: TcpStream,
    config: Arc<rustls::ServerConfig>,
) -> Result<()> {
    stream.set_nodelay(true)?;
    let mut stop = ctx.stop.subscribe();
    let auth = async {
        let stream = tokio::time::timeout(
            Duration::from_secs(5),
            TlsAcceptor::from(config).accept(stream),
        )
        .await??;
        authorize(ctx.clone(), TlsStream::Server(stream), false, None).await
    };
    let result = tokio::select! {_=stop.changed()=>return Ok(()),r=tokio::time::timeout(Duration::from_secs(310),auth)=>r??};
    if let Some((stream, fp)) = result {
        control(ctx, stream, fp, false, stop).await?;
    }
    Ok(())
}
async fn authorize(
    ctx: Rc<Context>,
    mut stream: TlsStream<TcpStream>,
    initiator: bool,
    pin: Option<Zeroizing<String>>,
) -> Result<Option<(TlsStream<TcpStream>, String)>> {
    let (_, connection) = stream.get_ref();
    ensure!(
        connection.alpn_protocol() == Some(b"ravel/1"),
        "Protocol negotiation failed"
    );
    let certificate = &connection
        .peer_certificates()
        .ok_or_else(|| anyhow!("Peer certificate required"))?[0];
    let fp = crate::identity::fingerprint(certificate);
    ensure!(
        fp != ctx.identity.fingerprint,
        "Cannot connect to this device itself"
    );
    let exporter = match &stream {
        TlsStream::Client(s) => {
            s.get_ref()
                .1
                .export_keying_material([0u8; 32], b"EXPORTER-Ravel-pair-v1", None)?
        }
        TlsStream::Server(s) => {
            s.get_ref()
                .1
                .export_keying_material([0u8; 32], b"EXPORTER-Ravel-pair-v1", None)?
        }
    };
    if initiator {
        protocol::write(
            &mut stream,
            &Message::Hello {
                version: VERSION,
                name: ctx.name.clone(),
                pairing: pin.is_some(),
            },
        )
        .await?;
    }
    let Message::Hello {
        version,
        name,
        pairing: requested_pairing,
    } = tokio::time::timeout(Duration::from_secs(5), protocol::read(&mut stream)).await??
    else {
        bail!("Expected protocol hello");
    };
    ensure!(version == VERSION, "Incompatible protocol version");
    validate_name(&name)?;
    let pairing = if initiator {
        ensure!(requested_pairing == pin.is_some(), "Pairing mode mismatch");
        pin.is_some()
    } else {
        requested_pairing
    };
    if !initiator {
        protocol::write(
            &mut stream,
            &Message::Hello {
                version: VERSION,
                name: ctx.name.clone(),
                pairing,
            },
        )
        .await?;
    }
    if !pairing {
        ensure!(ctx.enabled.get(), "Sharing stopped on this device");
        ensure!(
            ctx.store.borrow().trusted(&fp),
            "Unknown or revoked fingerprint; pair explicitly first"
        );
        return Ok(Some((stream, fp)));
    }
    let (client_fp, server_fp) = if initiator {
        (ctx.identity.fingerprint.as_str(), fp.as_str())
    } else {
        (fp.as_str(), ctx.identity.fingerprint.as_str())
    };
    let binding = Binding {
        client_fingerprint: client_fp,
        server_fingerprint: server_fp,
        exporter: &exporter,
    };
    let deadline = if let Some(pin) = pin {
        tokio::time::timeout(
            Duration::from_secs(15),
            pairing::exchange(&mut stream, &pin, binding, true),
        )
        .await??;
        Instant::now() + pairing::PIN_LIFETIME
    } else {
        let lease = ctx
            .offer
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| anyhow!("No pairing offer is open"))?
            .admit(Instant::now())?;
        let result = tokio::time::timeout(
            Duration::from_secs(15),
            pairing::exchange(&mut stream, &lease.pin, binding, false),
        )
        .await;
        let success = matches!(result, Ok(Ok(())));
        let finished = ctx
            .offer
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| anyhow!("Pairing offer closed"))?
            .finish(lease.generation, success, Instant::now());
        finished?;
        result??;
        let _ = ctx.events.try_send(Event::PinClosed);
        lease.deadline
    };
    let (reply, approval) = oneshot::channel();
    ctx.events
        .send(Event::Confirm {
            fingerprint: fp.clone(),
            name: name.clone(),
            deadline,
            reply,
        })
        .await
        .map_err(|_| anyhow!("GUI closed"))?;
    let approved =
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), approval).await??;
    ensure!(
        approved && Instant::now() < deadline,
        "Fingerprint rejected or PIN authorization expired"
    );
    protocol::write(&mut stream, &Message::Approved).await?;
    ensure!(
        matches!(
            tokio::time::timeout_at(
                tokio::time::Instant::from_std(deadline),
                protocol::read(&mut stream)
            )
            .await??,
            Message::Approved
        ),
        "Peer did not approve fingerprint"
    );
    ensure!(Instant::now() < deadline, "PIN authorization expired");
    ctx.store.borrow_mut().trust(fp.clone(), name)?;
    ctx.trusted_update();
    ctx.status(format!(
        "Paired with {}. Allow sharing on both devices, then connect.",
        crate::identity::pretty_fingerprint(&fp)
    ));
    Ok(None)
}
async fn control(
    ctx: Rc<Context>,
    stream: TlsStream<TcpStream>,
    fp: String,
    initiator: bool,
    stop: watch::Receiver<u64>,
) -> Result<()> {
    ensure!(ctx.enabled.get(), "Sharing stopped");
    let _permit = ctx
        .session
        .clone()
        .try_acquire_owned()
        .map_err(|_| anyhow!("Another control session is active"))?;
    let layout = ctx.store.borrow().layout(&fp);
    let (status_tx, mut status_rx) = mpsc::channel(16);
    let events = ctx.events.clone();
    let forwarder = tokio::task::spawn_local(async move {
        while let Some(status) = status_rx.recv().await {
            let _ = events.try_send(Event::Status(status));
        }
    });
    let result = session::run(
        stream,
        layout,
        initiator,
        Signals {
            stop,
            store: ctx.store.clone(),
            fingerprint: fp,
            status: status_tx,
        },
    )
    .await;
    forwarder.abort();
    ctx.trusted_update();
    ctx.status("Disconnected — held input released");
    result
}

pub fn request_stop(directory: &std::path::Path) -> Result<()> {
    crate::identity::atomic_private(&directory.join("stop.request"), b"stop", true)
}
