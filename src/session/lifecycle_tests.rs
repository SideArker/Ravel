use super::*;
use crate::{geometry::Side, input::TestTrace};
use tokio::io::DuplexStream;

struct Session {
    peer: DuplexStream,
    task: tokio::task::JoinHandle<Result<()>>,
    capture: mpsc::Sender<CaptureEvent>,
    trace: Rc<TestTrace>,
    stop: watch::Sender<u64>,
    store: Rc<RefCell<Store>>,
    _directory: tempfile::TempDir,
}
impl Session {
    async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::load(directory.path()).unwrap();
        let fp = "ab".repeat(32);
        store.trust(fp.clone(), "Test peer".into()).unwrap();
        let (stop, receiver) = watch::channel(0);
        let (status, mut statuses) = mpsc::channel(16);
        let (backend, capture, trace) = Backend::test();
        let (local, mut peer) = tokio::io::duplex(65536);
        let store = Rc::new(RefCell::new(store));
        let signals = Signals {
            stop: receiver,
            stop_request: stop.clone(),
            store: store.clone(),
            fingerprint: fp,
            status,
        };
        let task = tokio::task::spawn_local(run_with_backend(
            local,
            Layout::default(),
            true,
            signals,
            backend,
        ));
        let Message::Ready { display, layout } = protocol::read(&mut peer).await.unwrap() else {
            panic!("Expected Ready");
        };
        protocol::write(&mut peer, &Message::Ready { display, layout })
            .await
            .unwrap();
        assert!(statuses.recv().await.unwrap().starts_with("Connected"));
        trace
            .wait_for(|state| state.armed == Some(Side::Right))
            .await;
        Self {
            peer,
            task,
            capture,
            trace,
            stop,
            store,
            _directory: directory,
        }
    }
    async fn hold_inputs(&mut self) {
        protocol::write(&mut self.peer, &Message::Enter([0.2, 0.5]))
            .await
            .unwrap();
        protocol::write(
            &mut self.peer,
            &Message::Input(Input::Key {
                code: 30,
                down: true,
            }),
        )
        .await
        .unwrap();
        protocol::write(
            &mut self.peer,
            &Message::Input(Input::Button {
                code: 0x110,
                down: true,
            }),
        )
        .await
        .unwrap();
        self.trace
            .wait_for(|state| {
                state.injected.contains(&Input::Button {
                    code: 0x110,
                    down: true,
                })
            })
            .await;
        assert_eq!(self.trace.state.borrow().armed, None);
    }
    fn assert_released(trace: &TestTrace) {
        let state = trace.state.borrow();
        assert!(state.closed);
        assert_eq!(state.armed, None);
        assert!(state.injected.contains(&Input::Key {
            code: 30,
            down: false
        }));
        assert!(state.injected.contains(&Input::Button {
            code: 0x110,
            down: false
        }));
    }
}
#[tokio::test]
async fn disconnect_releases_inputs_and_disarms_capture() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut session = Session::start().await;
            session.hold_inputs().await;
            drop(session.peer);
            assert!(session.task.await.unwrap().is_err());
            Session::assert_released(&session.trace);
        })
        .await;
}
#[tokio::test]
async fn stop_releases_inputs_and_disarms_capture() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut session = Session::start().await;
            session.hold_inputs().await;
            session.stop.send_modify(|epoch| *epoch += 1);
            session.task.await.unwrap().unwrap();
            Session::assert_released(&session.trace);
        })
        .await;
}
#[tokio::test(start_paused = true)]
async fn timeout_releases_held_inputs() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut session = Session::start().await;
            session.hold_inputs().await;
            tokio::time::advance(Duration::from_secs(5)).await;
            let error = session.task.await.unwrap().unwrap_err();
            assert!(error.to_string().contains("Peer timeout"));
            Session::assert_released(&session.trace);
        })
        .await;
}
#[tokio::test]
async fn injection_error_still_releases_all_held_inputs() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut session = Session::start().await;
            session.hold_inputs().await;
            session.trace.state.borrow_mut().fail_next = true;
            protocol::write(
                &mut session.peer,
                &Message::Input(Input::Scroll {
                    axis: 0,
                    value: 120.,
                }),
            )
            .await
            .unwrap();
            let error = session.task.await.unwrap().unwrap_err();
            assert!(error.to_string().contains("Test injection failure"));
            Session::assert_released(&session.trace);
        })
        .await;
}
#[tokio::test]
async fn input_without_enter_is_rejected_without_injection() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut session = Session::start().await;
            protocol::write(
                &mut session.peer,
                &Message::Input(Input::Key {
                    code: 30,
                    down: true,
                }),
            )
            .await
            .unwrap();
            assert!(session.task.await.unwrap().is_err());
            let state = session.trace.state.borrow();
            assert!(state.closed);
            assert!(
                !state
                    .injected
                    .iter()
                    .any(|event| matches!(event, Input::Key { .. } | Input::Button { .. }))
            );
        })
        .await;
}
#[tokio::test]
async fn malformed_input_releases_held_inputs() {
    use tokio::io::AsyncWriteExt;
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut session = Session::start().await;
            session.hold_inputs().await;
            session
                .peer
                .write_u32(protocol::MAX_FRAME as u32 + 1)
                .await
                .unwrap();
            assert!(session.task.await.unwrap().is_err());
            Session::assert_released(&session.trace);
        })
        .await;
}
#[tokio::test]
async fn captured_motion_and_keyboard_use_the_control_stream() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut session = Session::start().await;
            session
                .capture
                .send(CaptureEvent::Begin {
                    point: [999., 400.],
                })
                .await
                .unwrap();
            let mut inputs = 0;
            session
                .capture
                .send(CaptureEvent::Input(Event::Pointer(PointerEvent::Motion {
                    time: 0,
                    dx: 100.,
                    dy: 50.,
                })))
                .await
                .unwrap();
            session
                .capture
                .send(CaptureEvent::Input(Event::Keyboard(KeyboardEvent::Key {
                    time: 0,
                    key: 30,
                    state: 1,
                })))
                .await
                .unwrap();
            while inputs < 3 {
                match protocol::read(&mut session.peer).await.unwrap() {
                    Message::Enter(point) => {
                        assert!(point_valid(point));
                        inputs += 1;
                    }
                    Message::Input(Input::Key {
                        code: 30,
                        down: true,
                    }) => inputs += 1,
                    Message::Input(Input::Motion { x, y }) => {
                        assert!(point_valid([x, y]));
                        inputs += 1;
                    }
                    Message::Ping => {}
                    _ => panic!("Unexpected capture message"),
                }
            }
            session.stop.send_modify(|epoch| *epoch += 1);
            session.task.await.unwrap().unwrap();
            assert!(session.trace.state.borrow().closed);
        })
        .await;
}

#[tokio::test(start_paused = true)]
async fn stalled_display_write_has_a_deadline_and_closes_backend() {
    stalled_display_exchange(false).await;
}
#[tokio::test(start_paused = true)]
async fn stop_interrupts_stalled_display_write_and_closes_backend() {
    stalled_display_exchange(true).await;
}
async fn stalled_display_exchange(stop_requested: bool) {
    tokio::task::LocalSet::new()
        .run_until(async {
            let directory = tempfile::tempdir().unwrap();
            let mut store = Store::load(directory.path()).unwrap();
            let fingerprint = "ab".repeat(32);
            store
                .trust(fingerprint.clone(), "Test peer".into())
                .unwrap();
            let (stop, receiver) = watch::channel(0);
            let (status, _) = mpsc::channel(16);
            let signals = Signals {
                stop: receiver,
                stop_request: stop.clone(),
                store: Rc::new(RefCell::new(store)),
                fingerprint,
                status,
            };
            // The peer neither reads nor writes. The first frame cannot fit.
            let (stream, _peer) = tokio::io::duplex(1);
            let (backend, _capture, trace) = Backend::test();
            let mut task = tokio::task::spawn_local(run_with_backend(
                stream,
                Layout::default(),
                true,
                signals,
                backend,
            ));
            tokio::task::yield_now().await;
            if stop_requested {
                stop.send_modify(|epoch| *epoch += 1);
            } else {
                tokio::time::advance(Duration::from_secs(6)).await;
            }
            let joined = tokio::time::timeout(Duration::from_secs(1), &mut task).await;
            if joined.is_err() {
                task.abort();
                panic!("Display write remained stuck after timeout or stop");
            }
            let result = joined.unwrap().unwrap();
            if stop_requested {
                assert!(result.is_ok());
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("Display exchange timed out")
                );
            }
            assert!(trace.state.borrow().closed);
            assert_eq!(trace.state.borrow().armed, None);
        })
        .await;
}

#[tokio::test]
async fn revoked_peer_cannot_inject_another_event_and_held_inputs_are_released() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut session = Session::start().await;
            session.hold_inputs().await;
            session.store.borrow_mut().revoke(&"ab".repeat(32)).unwrap();
            protocol::write(
                &mut session.peer,
                &Message::Input(Input::Key {
                    code: 31,
                    down: true,
                }),
            )
            .await
            .unwrap();
            let error = session.task.await.unwrap().unwrap_err();
            assert!(error.to_string().contains("Peer trust revoked"));
            Session::assert_released(&session.trace);
            assert!(
                !session.trace.state.borrow().injected.contains(&Input::Key {
                    code: 31,
                    down: true
                })
            );
        })
        .await;
}

#[tokio::test]
async fn leave_releases_inputs_and_restores_local_capture() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut session = Session::start().await;
            session.hold_inputs().await;
            protocol::write(&mut session.peer, &Message::Leave)
                .await
                .unwrap();
            session
                .trace
                .wait_for(|state| state.armed == Some(Side::Right))
                .await;
            assert!(session.trace.state.borrow().injected.contains(&Input::Key {
                code: 30,
                down: false
            }));
            assert!(
                session
                    .trace
                    .state
                    .borrow()
                    .injected
                    .contains(&Input::Button {
                        code: 0x110,
                        down: false
                    })
            );
            session.stop.send_modify(|epoch| *epoch += 1);
            session.task.await.unwrap().unwrap();
        })
        .await;
}
