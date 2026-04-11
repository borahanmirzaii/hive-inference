use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

/// Dashboard event — serialized as JSON and sent over WebSocket.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(dead_code)]
pub enum DashboardEvent {
    AgentConnected {
        agent_id: String,
        agent_name: String,
        addr: String,
        peer_count: usize,
    },
    Heartbeat {
        agent_id: String,
        load: f64,
        timestamp_ms: u64,
    },
    JobCreated {
        job_id: String,
        submitter: String,
        chunk_count: usize,
        timestamp_ms: u64,
    },
    BidSent {
        agent_id: String,
        job_id: String,
        chunk_index: usize,
        score: f64,
    },
    ChunkAssigned {
        job_id: String,
        chunk_index: usize,
        agent_id: String,
    },
    ChunkDone {
        agent_id: String,
        job_id: String,
        chunk_index: usize,
        processing_ms: u64,
        result_hash: String,
    },
    ResultReceived {
        job_id: String,
        chunk_index: usize,
        from_agent: String,
    },
    PocBuilt {
        job_id: String,
        poc_hash: String,
    },
    PocSigReceived {
        job_id: String,
        from_agent: String,
        sig_count: usize,
        total_agents: usize,
    },
    PocVerified {
        job_id: String,
        sig_count: usize,
        total_agents: usize,
    },
    StaleDetected {
        agent_id: String,
        stale_agent: String,
    },
    Redistributed {
        job_id: String,
        agent_id: String,
    },
    ReplayRejected {
        agent_id: String,
        from_agent: String,
        reason: String,
    },
}

pub async fn run_ws_server(addr: SocketAddr, event_tx: broadcast::Sender<DashboardEvent>) {
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[ws] failed to bind {addr}: {e}");
            return;
        }
    };

    eprintln!("[ws] dashboard server listening on ws://{addr}");

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[ws] accept error: {e}");
                continue;
            }
        };

        let rx = event_tx.subscribe();
        tokio::spawn(handle_connection(stream, peer, rx));
    }
}

async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    mut event_rx: broadcast::Receiver<DashboardEvent>,
) {
    let ws_stream = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("[ws] handshake error from {peer}: {e}");
            return;
        }
    };

    eprintln!("[ws] dashboard connected from {peer}");
    let (mut ws_tx, ws_rx) = ws_stream.split();

    // Keep the receive half alive in background (detects close)
    let mut recv_handle = tokio::spawn(async move {
        let mut rx = ws_rx;
        while let Some(msg) = rx.next().await {
            match msg {
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {} // ignore other client messages
            }
        }
    });

    // Send events until connection drops or channel closes
    loop {
        tokio::select! {
            result = event_rx.recv() => {
                match result {
                    Ok(event) => {
                        if let Ok(json) = serde_json::to_string(&event) {
                            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("[ws] {peer} lagged by {n} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = &mut recv_handle => {
                // Client disconnected
                break;
            }
        }
    }

    eprintln!("[ws] {peer} disconnected");
}
