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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::AgentIdentity;

    fn make_job(chunks: usize) -> InferenceJob {
        InferenceJob {
            id: "job-test".into(),
            submitter: "submitter".into(),
            chunks: (0..chunks).map(|i| format!("chunk {i}")).collect(),
            created_at_ms: 1000,
        }
    }

    fn make_bid(agent: &mut AgentIdentity, job_id: &str, chunk: usize, score: f64) -> SignedEnvelope<ChunkBid> {
        let bid = ChunkBid {
            job_id: job_id.into(),
            chunk_index: chunk,
            bidder: agent.id.clone(),
            capacity_score: score,
            timestamp_ms: 1000,
            nonce: 0,
        };
        agent.sign(bid).unwrap()
    }

    #[test]
    fn assigns_chunks_to_highest_scorer() {
        let job = make_job(3);
        let mut coord = JobCoordinator::new(job, 2000);

        let mut agent_a = AgentIdentity::generate();
        let mut agent_b = AgentIdentity::generate();

        // A bids higher on chunk 0, B bids higher on chunks 1 and 2
        coord.receive_bid(make_bid(&mut agent_a, "job-test", 0, 0.9));
        coord.receive_bid(make_bid(&mut agent_b, "job-test", 0, 0.5));
        coord.receive_bid(make_bid(&mut agent_a, "job-test", 1, 0.3));
        coord.receive_bid(make_bid(&mut agent_b, "job-test", 1, 0.8));
        coord.receive_bid(make_bid(&mut agent_a, "job-test", 2, 0.4));
        coord.receive_bid(make_bid(&mut agent_b, "job-test", 2, 0.7));

        let active = vec![agent_a.id.clone(), agent_b.id.clone()];
        let assignments = coord.resolve_assignments(&active);

        assert_eq!(assignments[&0], agent_a.id);
        assert_eq!(assignments[&1], agent_b.id);
        assert_eq!(assignments[&2], agent_b.id);
    }

    #[test]
    fn deterministic_tiebreak_by_agent_id() {
        let job = make_job(1);
        let mut coord = JobCoordinator::new(job, 2000);

        let mut agent_a = AgentIdentity::generate();
        let mut agent_b = AgentIdentity::generate();

        // Equal scores — tiebreak should pick lower agent_id
        coord.receive_bid(make_bid(&mut agent_a, "job-test", 0, 0.5));
        coord.receive_bid(make_bid(&mut agent_b, "job-test", 0, 0.5));

        let active = vec![agent_a.id.clone(), agent_b.id.clone()];
        let assignments = coord.resolve_assignments(&active);

        // Lower ID wins the tiebreak
        let expected = if agent_a.id < agent_b.id { &agent_a.id } else { &agent_b.id };
        assert_eq!(&assignments[&0], expected);
    }

    #[test]
    fn fallback_assigns_unbid_chunks() {
        let job = make_job(3);
        let mut coord = JobCoordinator::new(job, 2000);

        let mut agent_a = AgentIdentity::generate();

        // Only bid on chunk 0 — chunks 1 and 2 have no bids
        coord.receive_bid(make_bid(&mut agent_a, "job-test", 0, 0.9));

        let active = vec![agent_a.id.clone(), "agent-b".into()];
        let assignments = coord.resolve_assignments(&active);

        assert_eq!(assignments.len(), 3); // all chunks assigned
        assert_eq!(assignments[&0], agent_a.id); // bid winner
        // chunks 1 and 2 assigned via round-robin to sorted active agents
        assert!(assignments.contains_key(&1));
        assert!(assignments.contains_key(&2));
    }

    #[test]
    fn accepts_poc_sigs_in_assigned_state() {
        let job = make_job(1);
        let mut coord = JobCoordinator::new(job, 2000);

        let mut agent = AgentIdentity::generate();
        coord.receive_bid(make_bid(&mut agent, "job-test", 0, 0.9));
        coord.resolve_assignments(&[agent.id.clone()]);

        // Should accept sig even in Assigned state (before BuildingPoc)
        coord.receive_poc_sig("agent-x".into(), "sig-hex".into());
        assert_eq!(coord.poc_sigs.len(), 1);
    }
}
