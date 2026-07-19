//! Loopback bridge between the standalone InputMethodKit host and Quip's
//! existing composition/inference engine.

use quip_contracts::{CaptureResult, Rect, Trigger};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Manager};

pub(crate) const BRIDGE_ADDRESS: &str = "127.0.0.1:48731";
const NATIVE_DESTINATION_PREFIX: &str = "native_ime_";

type SharedWriter = Arc<Mutex<TcpStream>>;

#[derive(Clone)]
struct Route {
    writer: SharedWriter,
    session_id: String,
    generation: u64,
    destination_id: String,
}

#[derive(Default)]
struct Bridge {
    destinations: Mutex<HashMap<String, Route>>,
    bursts: Mutex<HashMap<String, Route>>,
    offered: Mutex<HashSet<String>>,
}

static BRIDGE: OnceLock<Arc<Bridge>> = OnceLock::new();

fn caret_is_usable(caret: &Rect) -> bool {
    caret.x.is_finite()
        && caret.y.is_finite()
        && caret.width.is_finite()
        && caret.height.is_finite()
        && (-10_000.0..=100_000.0).contains(&caret.x)
        && (-10_000.0..=100_000.0).contains(&caret.y)
        && (0.0..=200.0).contains(&caret.width)
        && (4.0..=200.0).contains(&caret.height)
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Capture {
        session_id: String,
        generation: u64,
        draft: String,
        caret: Rect,
    },
    Select {
        destination_id: String,
        index: usize,
    },
    Accept {
        destination_id: String,
    },
    Move {
        destination_id: String,
        delta: i64,
    },
    Dismiss {
        session_id: String,
    },
    Ping,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage<'a> {
    CaptureAccepted {
        session_id: &'a str,
        generation: u64,
        burst_id: &'a str,
        destination_id: &'a str,
    },
    Settled {
        session_id: &'a str,
        generation: u64,
        burst_id: &'a str,
        destination_id: &'a str,
        offered: bool,
    },
    Commit {
        session_id: &'a str,
        generation: u64,
        destination_id: &'a str,
        text: &'a str,
    },
    Dismissed {
        session_id: &'a str,
        destination_id: &'a str,
    },
    Pong,
    Error {
        message: &'a str,
    },
}

pub(crate) fn start(app: &AppHandle) -> Result<(), String> {
    if BRIDGE.get().is_some() {
        return Ok(());
    }
    let listener = TcpListener::bind(BRIDGE_ADDRESS).map_err(|error| {
        format!("could not bind native IME bridge at {BRIDGE_ADDRESS}: {error}")
    })?;
    let bridge = Arc::new(Bridge::default());
    BRIDGE
        .set(bridge.clone())
        .map_err(|_| "native IME bridge was already initialized".to_string())?;
    let app = app.clone();
    std::thread::Builder::new()
        .name("quip-ime-bridge".into())
        .spawn(move || {
            tracing::info!(address = BRIDGE_ADDRESS, "native IME bridge listening");
            for connection in listener.incoming() {
                match connection {
                    Ok(stream) => {
                        let app = app.clone();
                        let bridge = bridge.clone();
                        let _ = std::thread::Builder::new()
                            .name("quip-ime-client".into())
                            .spawn(move || handle_client(app, bridge, stream));
                    }
                    Err(error) => tracing::warn!(%error, "native IME bridge accept failed"),
                }
            }
        })
        .map_err(|error| format!("could not start native IME bridge thread: {error}"))?;
    Ok(())
}

fn handle_client(app: AppHandle, bridge: Arc<Bridge>, stream: TcpStream) {
    let peer = stream.peer_addr().ok();
    let writer = match stream.try_clone() {
        Ok(stream) => Arc::new(Mutex::new(stream)),
        Err(error) => {
            tracing::warn!(%error, "could not clone native IME bridge stream");
            return;
        }
    };
    tracing::info!(?peer, "native IME bridge client connected");
    for line in BufReader::new(stream).lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                tracing::warn!(%error, "native IME bridge read failed");
                break;
            }
        };
        let message = match serde_json::from_str::<ClientMessage>(&line) {
            Ok(message) => message,
            Err(error) => {
                tracing::warn!(%error, "native IME bridge rejected malformed message");
                let _ = send(
                    &writer,
                    &ServerMessage::Error {
                        message: "malformed_message",
                    },
                );
                continue;
            }
        };
        handle_message(&app, &bridge, &writer, message);
    }
    let disconnected_destinations = {
        let mut destinations = bridge.destinations.lock().unwrap();
        let disconnected = destinations
            .iter()
            .filter_map(|(destination_id, route)| {
                Arc::ptr_eq(&route.writer, &writer).then_some(destination_id.clone())
            })
            .collect::<HashSet<_>>();
        destinations.retain(|_, route| !Arc::ptr_eq(&route.writer, &writer));
        disconnected
    };
    bridge
        .bursts
        .lock()
        .unwrap()
        .retain(|_, route| !Arc::ptr_eq(&route.writer, &writer));
    bridge
        .offered
        .lock()
        .unwrap()
        .retain(|destination_id| !disconnected_destinations.contains(destination_id));
    tracing::info!(?peer, "native IME bridge client disconnected");
}

fn handle_message(
    app: &AppHandle,
    bridge: &Arc<Bridge>,
    writer: &SharedWriter,
    message: ClientMessage,
) {
    match message {
        ClientMessage::Capture {
            session_id,
            generation,
            draft,
            mut caret,
        } => {
            if !caret_is_usable(&caret) {
                if let Some(accessibility_caret) =
                    crate::accessibility::focused_caret_rect().filter(caret_is_usable)
                {
                    tracing::debug!(
                        session_id,
                        generation,
                        "replaced invalid native caret with Accessibility geometry"
                    );
                    caret = accessibility_caret;
                } else {
                    tracing::warn!(
                        session_id,
                        generation,
                        "native and Accessibility caret geometry unavailable"
                    );
                    caret = Rect {
                        x: 512.0,
                        y: 384.0,
                        width: 1.0,
                        height: 18.0,
                    };
                }
            }
            let safe_session: String = session_id
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() {
                        character
                    } else {
                        '_'
                    }
                })
                .collect();
            let burst_id = format!("native_burst_{safe_session}_{generation}");
            let destination_id = format!("{NATIVE_DESTINATION_PREFIX}{safe_session}_{generation}");
            let route = Route {
                writer: writer.clone(),
                session_id: session_id.clone(),
                generation,
                destination_id: destination_id.clone(),
            };
            bridge
                .destinations
                .lock()
                .unwrap()
                .insert(destination_id.clone(), route.clone());
            bridge
                .bursts
                .lock()
                .unwrap()
                .insert(burst_id.clone(), route);
            let _ = send(
                writer,
                &ServerMessage::CaptureAccepted {
                    session_id: &session_id,
                    generation,
                    burst_id: &burst_id,
                    destination_id: &destination_id,
                },
            );
            let (profile_id, capture_context) = {
                let engine = app.state::<crate::EngineState>();
                let engine = engine.0.lock().unwrap();
                (
                    engine.settings.active_profile.clone(),
                    engine.settings.should_capture_context(),
                )
            };
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                crate::run_capture_result(
                    handle,
                    CaptureResult::Ready {
                        burst_id,
                        destination_id,
                        profile_id,
                        draft,
                        trigger: Trigger::Idle,
                        caret,
                        word_offset: None,
                    },
                    capture_context,
                    "native_ime_bridge",
                    None,
                    None,
                )
                .await;
            });
        }
        ClientMessage::Select {
            destination_id,
            index,
        } => {
            if !bridge
                .destinations
                .lock()
                .unwrap()
                .contains_key(&destination_id)
            {
                let _ = send(
                    writer,
                    &ServerMessage::Error {
                        message: "unknown_destination",
                    },
                );
                return;
            }
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Err(error) = crate::select_candidate(handle, index) {
                    tracing::warn!(%error, index, "native IME candidate selection failed");
                }
            });
        }
        ClientMessage::Accept { destination_id } => {
            if !bridge
                .destinations
                .lock()
                .unwrap()
                .contains_key(&destination_id)
            {
                let _ = send(
                    writer,
                    &ServerMessage::Error {
                        message: "unknown_destination",
                    },
                );
                return;
            }
            let index = {
                let engine = app.state::<crate::EngineState>();
                let engine = engine.0.lock().unwrap();
                match engine.current_snapshot() {
                    crate::composition::Snapshot::Suggesting { selected, .. } => Some(selected),
                    _ => None,
                }
            };
            let Some(index) = index else {
                let _ = send(
                    writer,
                    &ServerMessage::Error {
                        message: "no_active_offer",
                    },
                );
                return;
            };
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Err(error) = crate::select_candidate(handle, index) {
                    tracing::warn!(%error, index, "native IME highlighted candidate acceptance failed");
                }
            });
        }
        ClientMessage::Move {
            destination_id,
            delta,
        } => {
            if !bridge
                .destinations
                .lock()
                .unwrap()
                .contains_key(&destination_id)
            {
                let _ = send(
                    writer,
                    &ServerMessage::Error {
                        message: "unknown_destination",
                    },
                );
                return;
            }
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || crate::move_selection(handle, delta));
        }
        ClientMessage::Dismiss { session_id } => {
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || {
                tracing::debug!(%session_id, "native IME dismissed suggestions");
                crate::dismiss_suggestions(handle);
            });
        }
        ClientMessage::Ping => {
            let _ = send(writer, &ServerMessage::Pong);
        }
    }
}

fn send(writer: &SharedWriter, message: &ServerMessage<'_>) -> Result<(), String> {
    let mut writer = writer
        .lock()
        .map_err(|_| "IME bridge writer poisoned".to_string())?;
    serde_json::to_writer(&mut *writer, message)
        .map_err(|error| format!("could not encode IME bridge message: {error}"))?;
    writer
        .write_all(b"\n")
        .and_then(|_| writer.flush())
        .map_err(|error| format!("could not write IME bridge message: {error}"))
}

pub(crate) fn is_native_destination(destination_id: &str) -> bool {
    destination_id.starts_with(NATIVE_DESTINATION_PREFIX)
}

pub(crate) fn prediction_settled(burst_id: &str, offered: bool) {
    let Some(bridge) = BRIDGE.get() else { return };
    let Some(route) = bridge.bursts.lock().unwrap().get(burst_id).cloned() else {
        return;
    };
    if offered {
        bridge
            .offered
            .lock()
            .unwrap()
            .insert(route.destination_id.clone());
    }
    let _ = send(
        &route.writer,
        &ServerMessage::Settled {
            session_id: &route.session_id,
            generation: route.generation,
            burst_id,
            destination_id: &route.destination_id,
            offered,
        },
    );
}

pub(crate) fn commit_candidate(destination_id: &str, text: &str) -> Result<(), String> {
    let bridge = BRIDGE
        .get()
        .ok_or_else(|| "native IME bridge is not running".to_string())?;
    let route = bridge
        .destinations
        .lock()
        .unwrap()
        .get(destination_id)
        .cloned()
        .ok_or_else(|| "native IME destination is no longer connected".to_string())?;
    send(
        &route.writer,
        &ServerMessage::Commit {
            session_id: &route.session_id,
            generation: route.generation,
            destination_id,
            text,
        },
    )?;
    bridge.offered.lock().unwrap().remove(destination_id);
    Ok(())
}

pub(crate) fn dismiss_active() {
    let Some(bridge) = BRIDGE.get() else { return };
    let destinations: Vec<String> = bridge.offered.lock().unwrap().drain().collect();
    for destination_id in destinations {
        let route = bridge
            .destinations
            .lock()
            .unwrap()
            .get(&destination_id)
            .cloned();
        if let Some(route) = route {
            let _ = send(
                &route.writer,
                &ServerMessage::Dismissed {
                    session_id: &route.session_id,
                    destination_id: &route.destination_id,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_native_capture_contract() {
        let message: ClientMessage = serde_json::from_str(
            r#"{"type":"capture","session_id":"p1-c2","generation":7,"draft":"cnt cm tmrw","caret":{"x":10.0,"y":20.0,"width":1.0,"height":18.0}}"#,
        )
        .unwrap();
        assert!(matches!(
            message,
            ClientMessage::Capture { generation: 7, draft, .. } if draft == "cnt cm tmrw"
        ));
    }

    #[test]
    fn serializes_commit_contract() {
        let encoded = serde_json::to_string(&ServerMessage::Commit {
            session_id: "p1-c2",
            generation: 7,
            destination_id: "native_ime_p1_c2_7",
            text: "Can't come tomorrow.",
        })
        .unwrap();
        assert!(encoded.contains("\"type\":\"commit\""));
        assert!(encoded.contains("Can't come tomorrow."));
    }

    #[test]
    fn rejects_garbage_native_caret_geometry() {
        assert!(!caret_is_usable(&Rect {
            x: 1.6e-314,
            y: -100_340.0,
            width: 1.0,
            height: 1.0,
        }));
        assert!(caret_is_usable(&Rect {
            x: 465.0,
            y: 244.0,
            width: 1.0,
            height: 16.0,
        }));
    }
}
