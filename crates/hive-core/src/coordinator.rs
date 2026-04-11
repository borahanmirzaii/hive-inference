use std::collections::HashMap;

use crate::types::*;

#[derive(Debug, Clone, PartialEq)]
pub enum JobState {
    CollectingBids,
    Assigned(HashMap<ChunkIndex, AgentId>),
    CollectingResults,
    BuildingPoc {
        assignments: HashMap<ChunkIndex, AgentId>,
        poc_timestamp_ms: u64,
        poc_hash: String,
    },
    Complete,
    Failed(String),
}

pub struct JobCoordinator {
    pub job: InferenceJob,
    pub bids: Vec<SignedEnvelope<ChunkBid>>,
    pub results: Vec<SignedEnvelope<ChunkResult>>,
    pub poc_sigs: Vec<(AgentId, String)>,
    pub state: JobState,
    pub bid_deadline_ms: u64,
}

impl JobCoordinator {
    pub fn new(job: InferenceJob, bid_window_ms: u64) -> Self {
        let deadline = job.created_at_ms + bid_window_ms;
        Self {
            job,
            bids: vec![],
            results: vec![],
            poc_sigs: vec![],
            state: JobState::CollectingBids,
            bid_deadline_ms: deadline,
        }
    }

    pub fn receive_bid(&mut self, bid: SignedEnvelope<ChunkBid>) {
        if matches!(self.state, JobState::CollectingBids) {
            self.bids.push(bid);
        }
    }

    pub fn receive_result(&mut self, result: SignedEnvelope<ChunkResult>) {
        match &self.state {
            JobState::Assigned(_) | JobState::CollectingResults | JobState::BuildingPoc { .. } => {
                self.results.push(result);
            }
            _ => {}
        }
    }

    pub fn receive_poc_sig(&mut self, agent_id: AgentId, signature: String) {
        // Accept sigs in Assigned (results may still be arriving) and BuildingPoc states.
        // PocContributions can arrive before an agent has processed all ChunkResults.
        match &self.state {
            JobState::Assigned(_) | JobState::BuildingPoc { .. } => {
                if !self.poc_sigs.iter().any(|(id, _)| id == &agent_id) {
                    self.poc_sigs.push((agent_id, signature));
                }
            }
            _ => {}
        }
    }

    /// Deterministic assignment — every node computes identically because
    /// Vertex guarantees the same bid ordering on all nodes.
    ///
    /// For each chunk: pick the bid with highest capacity_score.
    /// Tie-break: lowest agent_id (deterministic string comparison).
    /// Fallback: unassigned chunks round-robin to agents with lowest load.
    pub fn resolve_assignments(
        &mut self,
        active_agents: &[AgentId],
    ) -> HashMap<ChunkIndex, AgentId> {
        let num_chunks = self.job.chunks.len();
        let mut assignments: HashMap<ChunkIndex, AgentId> = HashMap::new();

        // For each chunk, find the best bid
        for chunk_idx in 0..num_chunks {
            let best_bid = self
                .bids
                .iter()
                .filter(|b| b.payload.chunk_index == chunk_idx)
                .max_by(|a, b| {
                    a.payload
                        .capacity_score
                        .partial_cmp(&b.payload.capacity_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| b.payload.bidder.cmp(&a.payload.bidder))
                });

            if let Some(bid) = best_bid {
                assignments.insert(chunk_idx, bid.payload.bidder.clone());
            }
        }

        // Fallback: unassigned chunks get round-robin'd to active agents
        if !active_agents.is_empty() {
            let mut sorted_active = active_agents.to_vec();
            sorted_active.sort();
            let mut rr_idx = 0usize;
            for chunk_idx in 0..num_chunks {
                if !assignments.contains_key(&chunk_idx) {
                    assignments.insert(
                        chunk_idx,
                        sorted_active[rr_idx % sorted_active.len()].clone(),
                    );
                    rr_idx += 1;
                }
            }
        }

        self.state = JobState::Assigned(assignments.clone());
        assignments
    }

    pub fn all_results_in(&self) -> bool {
        match &self.state {
            JobState::Assigned(ref assignments) => self.results.len() >= assignments.len(),
            JobState::BuildingPoc { ref assignments, .. } => {
                self.results.len() >= assignments.len()
            }
            _ => false,
        }
    }

    pub fn expected_chunks(&self) -> usize {
        self.job.chunks.len()
    }

    pub fn transition_to_collecting_results(&mut self) {
        if let JobState::Assigned(_) = &self.state {
            self.state = JobState::CollectingResults;
        }
    }

    pub fn transition_to_building_poc(
        &mut self,
        assignments: HashMap<ChunkIndex, AgentId>,
        poc_timestamp_ms: u64,
        poc_hash: String,
    ) {
        self.state = JobState::BuildingPoc {
            assignments,
            poc_timestamp_ms,
            poc_hash,
        };
    }

    pub fn transition_to_complete(&mut self) {
        self.state = JobState::Complete;
    }

    pub fn transition_to_failed(&mut self, reason: String) {
        self.state = JobState::Failed(reason);
    }
}
