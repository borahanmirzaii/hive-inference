use serde::{Deserialize, Serialize};

pub type AgentId = String;
pub type JobId = String;
pub type ChunkIndex = usize;

/// A document split into chunks for distributed processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceJob {
    pub id: JobId,
    pub submitter: AgentId,
    pub chunks: Vec<String>,
    pub created_at_ms: u64,
}

/// An agent's claim on one chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkBid {
    pub job_id: JobId,
    pub chunk_index: ChunkIndex,
    pub bidder: AgentId,
    pub capacity_score: f64,
    pub timestamp_ms: u64,
    pub nonce: u64,
}

/// Result of processing one chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkResult {
    pub job_id: JobId,
    pub chunk_index: ChunkIndex,
    pub agent_id: AgentId,
    pub result: String,
    pub result_hash: String,
    pub processing_ms: u64,
    pub timestamp_ms: u64,
    pub nonce: u64,
}

/// Heartbeat — proves agent is alive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub agent_id: AgentId,
    pub timestamp_ms: u64,
    pub nonce: u64,
    pub load: f64,
}

/// Cryptographic Proof of Coordination for one completed job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofOfCoordination {
    pub job_id: JobId,
    pub participants: Vec<AgentId>,
    pub chunk_assignments: Vec<(AgentId, ChunkIndex)>,
    pub result_hashes: Vec<(AgentId, String)>,
    pub consensus_timestamp_ms: u64,
    pub poc_hash: String,
    pub signatures: Vec<(AgentId, String)>,
}

/// Wrapper that adds Ed25519 signature to any payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedEnvelope<T> {
    pub payload: T,
    pub signer_id: AgentId,
    pub nonce: u64,
    pub timestamp_ms: u64,
    pub signature: String,
}

/// Payload for a PoC signature contribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PocContributionPayload {
    pub job_id: JobId,
    pub agent_id: AgentId,
    pub poc_hash: String,
    pub signature_hex: String,
}

/// All message types sent over Vertex.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HiveMessage {
    Heartbeat(SignedEnvelope<Heartbeat>),
    JobAnnouncement(SignedEnvelope<InferenceJob>),
    ChunkBid(SignedEnvelope<ChunkBid>),
    ChunkResult(SignedEnvelope<ChunkResult>),
    PocContribution(SignedEnvelope<PocContributionPayload>),
    ChunkReassignment(SignedEnvelope<ChunkReassignmentPayload>),
}

/// Payload for chunk reassignment after fault detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkReassignmentPayload {
    pub job_id: JobId,
    pub stale_agent: AgentId,
    pub new_assignments: Vec<(ChunkIndex, AgentId)>,
    pub timestamp_ms: u64,
    pub nonce: u64,
}
