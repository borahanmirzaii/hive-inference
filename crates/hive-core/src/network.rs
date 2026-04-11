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

        // Tune Vertex for low-latency local swarm coordination
        let mut options = Options::default();
        options.set_heartbeat_us(500_000); // 500ms Vertex heartbeat (fast gossip)
        options.set_target_ack_latency_ms(400); // tighten ack target
        options.set_max_ack_latency_ms(800); // cap ack latency
        options.set_enable_dynamic_epoch_size(true); // adapt to swarm size changes
        options.set_transaction_channel_size(4096); // buffer for burst traffic

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

    /// Receive next consensus-ordered HiveMessages.
    /// Returns all messages from one Event, or empty vec on timeout.
    /// Uses a timeout to avoid blocking the select! loop indefinitely.
    pub async fn recv(&self) -> Result<Vec<HiveMessage>, NetworkError> {
        // Short timeout so the caller's select! loop can fire timers (heartbeats, etc.)
        match tokio::time::timeout(
            std::time::Duration::from_millis(100),
            self.recv_inner(),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Ok(vec![]), // timeout — no messages, let caller do other work
        }
    }

    async fn recv_inner(&self) -> Result<Vec<HiveMessage>, NetworkError> {
        match self.engine.recv_message().await? {
            Some(Message::Event(event)) => {
                let mut msgs = Vec::new();
                for tx in event.transactions() {
                    if let Ok(msg) = serde_json::from_slice::<HiveMessage>(tx) {
                        msgs.push(msg);
                    }
                }
                Ok(msgs)
            }
            Some(Message::SyncPoint(_)) => Ok(vec![]),
            None => Ok(vec![]),
        }
    }
}
