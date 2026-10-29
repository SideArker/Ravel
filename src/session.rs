use crate::{
    geometry::{Display, Layout},
    input::{Backend, CaptureEvent, Held},
    protocol::{self, Input, Message, point_valid},
    store::Store,
};
use anyhow::{Result, anyhow, bail, ensure};
use input_event::{Event, KeyboardEvent, PointerEvent};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, watch},
};

pub struct Signals {
    pub stop: watch::Receiver<u64>,
    pub stop_request: watch::Sender<u64>,
    pub store: Rc<RefCell<Store>>,
    pub fingerprint: String,
    pub status: mpsc::Sender<String>,
}
#[derive(Default, Debug, PartialEq)]
pub enum Direction {
    #[default]
    Local,
    Sending,
    Receiving,
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
    mut stream: S,
    layout: Layout,
    initiator: bool,
    mut signals: Signals,
) -> Result<()> {
    // No OS capture/injection object exists before network authorization.
    ensure!(
        signals.store.borrow().trusted(&signals.fingerprint),
        "Peer is not trusted"
    );
    ensure!(!signals.stop.has_changed()?, "Sharing was stopped");
    let mut backend = Backend::new().await?;
    let result = async {
        protocol::write(
            &mut stream,
            &Message::Ready {
                display: backend.display,
                layout,
            },
        )
        .await?;
        let Message::Ready {
            display: remote_display,
            layout: remote_layout,
        } = tokio::time::timeout(Duration::from_secs(5), protocol::read(&mut stream)).await??
        else {
            bail!("Expected display configuration");
        };
        remote_display.validate()?;
        remote_layout.validate()?;
        let layout = if initiator {
            layout
        } else {
            remote_layout.reciprocal()
        };
        layout.validate()?;
        signals
            .store
            .borrow_mut()
            .set_layout(&signals.fingerprint, layout)?;
        let _ = signals.status.try_send(format!(
            "Connected — {}. Move to the {:?} edge to share.",
            backend.label, layout.side
        ));
        control(stream, &mut backend, remote_display, layout, &mut signals).await
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

async fn control<S: AsyncRead + AsyncWrite + Unpin + 'static>(
    stream: S,
    backend: &mut Backend,
    remote: Display,
    layout: Layout,
    signals: &mut Signals,
) -> Result<()> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (incoming_tx, mut incoming) = mpsc::channel::<Result<Message>>(128);
    let (outgoing, mut outgoing_rx) = mpsc::channel(128);
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
        while let Some(message) = outgoing_rx.recv().await {
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
        }
    });
    let _tasks = Tasks { reader, writer };
    let mut direction = Direction::Local;
    let mut point = [0.5, 0.5];
    let mut held = Held::default();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
    let mut tick = tokio::time::interval(Duration::from_millis(25));
    let mut rearm_at = None;
    backend.arm(Some(layout.side)).await?;
    loop {
        tokio::select! {
            _=signals.stop.changed()=>{let _=send(&outgoing,Message::Stop);return Ok(());},
            message=incoming.recv()=>{
                ensure!(signals.store.borrow().trusted(&signals.fingerprint),"Peer trust revoked");
                ensure!(!signals.stop.has_changed()?,"Sharing stopped");
                match message.ok_or_else(||anyhow!("Connection closed"))?? {
                    Message::Enter(p)=>{ensure!(point_valid(p),"Invalid entry point");direction.receive_enter()?;backend.arm(None).await?;backend.inject(Input::Motion{x:p[0],y:p[1]})?;let _=signals.status.try_send("Receiving remote input — Disconnect stops sharing".into());},
                    Message::Input(event)=>{direction.receive_input()?;backend.inject(event.validate()?)?;},
                    Message::Leave=>{direction.receive_leave()?;backend.release_input()?;rearm_at=Some(Instant::now()+Duration::from_millis(350));let _=signals.status.try_send("Connected — local control".into());},
                    Message::Ping=>{},Message::Stop=>{signals.stop_request.send_modify(|e| *e=e.wrapping_add(1));return Ok(());},
                    _=>bail!("Unexpected message in control session"),
                }
            },
            event=backend.next()=>{
                let event=event?;
                match event {
                    CaptureEvent::Begin{point:entry}=>{
                        if direction!=Direction::Local || rearm_at.is_some(){backend.release_capture().await?;continue;}
                        if let Some(mapped)=layout.enter(backend.display.normalize(entry)) {
                            point=mapped;direction=Direction::Sending;send(&outgoing,Message::Enter(point))?;
                            let _=signals.status.try_send("Sending input — Ctrl+Alt+Shift+Esc stops sharing".into());
                        }else{backend.arm(None).await?;rearm_at=Some(Instant::now()+Duration::from_millis(350));}
                    },
                    CaptureEvent::Input(event)=>{
                        if direction!=Direction::Sending {continue;}
                        if let Event::Pointer(PointerEvent::Motion{dx,dy,..})=event {
                            ensure!(dx.is_finite() && dy.is_finite() && dx.abs()<=32768. && dy.abs()<=32768.,"Invalid captured motion");
                            point[0]+=dx/(remote.width-1.);point[1]+=dy/(remote.height-1.);
                            if layout.returns(point) {
                                send(&outgoing,Message::Leave)?;backend.arm(None).await?;
                                let p=layout.return_point([point[0].clamp(0.,1.),point[1].clamp(0.,1.)]);
                                backend.inject(Input::Motion{x:p[0],y:p[1]})?;held=Held::default();direction=Direction::Local;rearm_at=Some(Instant::now()+Duration::from_millis(350));
                            }else{
                                point=[point[0].clamp(0.,1.),point[1].clamp(0.,1.)];send(&outgoing,Message::Input(Input::Motion{x:point[0],y:point[1]}))?;
                            }
                        }else if let Some(input)=convert(event) {
                            input.validate()?;held.update(input);if held.emergency_stop(){signals.stop_request.send_modify(|e| *e=e.wrapping_add(1));let _=send(&outgoing,Message::Stop);return Ok(());}
                            send(&outgoing,Message::Input(input))?;
                        }
                    }
                }
            },
            _=heartbeat.tick()=>{backend.check_geometry()?;ensure!(signals.store.borrow().trusted(&signals.fingerprint),"Peer trust revoked");send(&outgoing,Message::Ping)?;},
            _=tick.tick()=>{if rearm_at.is_some_and(|t|Instant::now()>=t) && direction==Direction::Local{backend.arm(Some(layout.side)).await?;rearm_at=None;}},
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

#[cfg(test)]
mod tests {
    use super::*;
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
