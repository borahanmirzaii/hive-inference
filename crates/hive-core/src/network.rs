use std::sync::Arc;
use thiserror::Error;

use tashi_vertex::{
    Context, Engine, KeyPublic, KeySecret, Message, Options, Peers, Socket, Transaction,
};

use crate::types::HiveMessage;

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("vertex error: {0}")]
    Vertex(#[from] tashi_vertex::Error),
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("timeout binding socket")]
    BindTimeout,
    #[error("key parse error: {0}")]
    KeyParse(String),
}

/// Peer configuration using Vertex Base58-encoded public keys.
#[derive(Debug, Clone)]
pub struct PeerConfig {
    pub addr: String,
    /// Base58-encoded Vertex public key (ECDSA prime256v1, NOT Ed25519).
    pub vertex_pubkey: String,
}

pub struct VertexNode {
    engine: Arc<Engine>,
    _context: Arc<Context>,
    pub local_addr: String,
}

impl VertexNode {
    /// Start a Vertex node. Keys are ECDSA (Vertex-native), not Ed25519.
    /// `secret` is a Base58-encoded Vertex secret key.
    /// `peers` contains Base58-encoded Vertex public keys for each peer.
    pub async fn start(
        bind_addr: &str,
        secret: &KeySecret,
        peers: &[PeerConfig],
    ) -> Result<Self, NetworkError> {
        let mut peer_set = Peers::new()?;

        for p in peers {
            let pubkey: KeyPublic = p
                .vertex_pubkey
                .parse()
                .map_err(|e: tashi_vertex::Error| NetworkError::KeyParse(e.to_string()))?;
            peer_set.insert(&p.addr, &pubkey, Default::default())?;
        }

        // Add self
        peer_set.insert(bind_addr, &secret.public(), Default::default())?;

        let context = Context::new()?;
        let socket = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            Socket::bind(&context, bind_addr),
        )
        .await
        .map_err(|_| NetworkError::BindTimeout)??;

        // Tune Vertex for swarm coordination with burst traffic
        let mut options = Options::default();
        options.set_max_ack_latency_ms(800); // raise from 600ms default for burst tolerance
        options.set_transaction_channel_size(4096); // 128x default (32) for bid/result bursts
        options.set_base_min_event_interval_us(10_000); // 10ms min between events (default 50ms)

        let engine = Engine::start(
            &context,
            socket,
            options,
            secret,
            peer_set,
            false, // not joining a running session
        )?;

        Ok(VertexNode {
            engine: Arc::new(engine),
            _context: Arc::new(context),
            local_addr: bind_addr.to_string(),
        })
    }

    pub fn broadcast(&self, msg: &HiveMessage) -> Result<(), NetworkError> {
        let data = serde_json::to_vec(msg)?;
        let mut tx = Transaction::allocate(data.len());
        tx.copy_from_slice(&data);
        self.engine.send_transaction(tx)?;
        Ok(())
    }

    /// Receive next consensus-ordered HiveMessages with Vertex consensus timestamp.
    /// This blocks until a message arrives — use inside tokio::select! with other timers.
    pub async fn recv(&self) -> Result<ConsensusEvent, NetworkError> {
        match self.engine.recv_message().await? {
            Some(Message::Event(event)) => {
                let consensus_timestamp_ms = event.consensus_at();
                let event_hash = hex::encode(event.hash());
                let mut msgs = Vec::new();
                for tx in event.transactions() {
                    if let Ok(msg) = serde_json::from_slice::<HiveMessage>(tx) {
                        msgs.push(msg);
                    }
                }
                Ok(ConsensusEvent {
                    messages: msgs,
                    consensus_timestamp_ms,
                    event_hash,
                })
            }
            Some(Message::SyncPoint(_)) => Ok(ConsensusEvent::empty()),
            None => Ok(ConsensusEvent::empty()),
        }
    }
}

/// A batch of messages from a single Vertex consensus event.
pub struct ConsensusEvent {
    pub messages: Vec<HiveMessage>,
    /// Vertex-provided consensus timestamp (same on all nodes for this event).
    pub consensus_timestamp_ms: u64,
    /// Vertex event hash (cryptographic, consensus-derived).
    pub event_hash: String,
}

impl ConsensusEvent {
    pub fn empty() -> Self {
        Self {
            messages: vec![],
            consensus_timestamp_ms: 0,
            event_hash: String::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}
