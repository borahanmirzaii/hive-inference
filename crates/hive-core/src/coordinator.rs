use std::collections::HashMap;

use crate::types::*;

#[derive(Debug, Clone, PartialEq)]
pub enum JobState {
    CollectingBids,
    Assigned(HashMap<ChunkIndex, AgentId>),
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
    resolved: bool, // guard against double-resolution
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
            resolved: false,
        }
    }

    pub fn receive_bid(&mut self, bid: SignedEnvelope<ChunkBid>) {
        if matches!(self.state, JobState::CollectingBids) {
            self.bids.push(bid);
        }
    }

    pub fn receive_result(&mut self, result: SignedEnvelope<ChunkResult>) {
        // Accept results in any active state (not Complete/Failed)
        match &self.state {
            JobState::Complete | JobState::Failed(_) | JobState::CollectingBids => {}
            _ => {
                // Deduplicate: don't accept same chunk from same agent twice
                let dominated = self.results.iter().any(|r| {
                    r.payload.chunk_index == result.payload.chunk_index
                        && r.payload.agent_id == result.payload.agent_id
                });
                if !dominated {
                    self.results.push(result);
                }
            }
        }
    }

    pub fn receive_poc_sig(&mut self, agent_id: AgentId, signature: String) {
        // Accept sigs in ANY active state — PocContributions can arrive
        // before an agent has processed all ChunkResults (Vertex ordering).
        match &self.state {
            JobState::Complete | JobState::Failed(_) | JobState::CollectingBids => {}
            _ => {
                if !self.poc_sigs.iter().any(|(id, _)| id == &agent_id) {
                    self.poc_sigs.push((agent_id, signature));
                }
            }
        }
    }

    /// Deterministic assignment — every node computes identically because
    /// Vertex guarantees the same bid ordering on all nodes.
    ///
    /// Returns None if already resolved (guard against double-resolution).
    pub fn resolve_assignments(
        &mut self,
        active_agents: &[AgentId],
    ) -> Option<HashMap<ChunkIndex, AgentId>> {
        // Guard: only resolve once
        if self.resolved {
            return None;
        }
        if !matches!(self.state, JobState::CollectingBids) {
            return None;
        }

        self.resolved = true;
        let num_chunks = self.job.chunks.len();
        let mut assignments: HashMap<ChunkIndex, AgentId> = HashMap::new();

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

        // Fallback: unassigned chunks round-robin to sorted active agents
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
        Some(assignments)
    }

    pub fn get_assignments(&self) -> Option<&HashMap<ChunkIndex, AgentId>> {
        match &self.state {
            JobState::Assigned(a) => Some(a),
            JobState::BuildingPoc { assignments, .. } => Some(assignments),
            _ => None,
        }
    }

    pub fn results_complete(&self) -> bool {
        match &self.state {
            JobState::Assigned(assignments) => self.results.len() >= assignments.len(),
            _ => false,
        }
    }

    pub fn expected_chunks(&self) -> usize {
        self.job.chunks.len()
    }

    pub fn transition_to_building_poc(
        &mut self,
        assignments: HashMap<ChunkIndex, AgentId>,
        poc_timestamp_ms: u64,
        poc_hash: String,
    ) {
        if matches!(self.state, JobState::Assigned(_)) {
            self.state = JobState::BuildingPoc {
                assignments,
                poc_timestamp_ms,
                poc_hash,
            };
        }
    }

    pub fn transition_to_complete(&mut self) {
        self.state = JobState::Complete;
    }

    pub fn transition_to_failed(&mut self, reason: String) {
        self.state = JobState::Failed(reason);
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.state, JobState::Complete | JobState::Failed(_))
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

        coord.receive_bid(make_bid(&mut agent_a, "job-test", 0, 0.9));
        coord.receive_bid(make_bid(&mut agent_b, "job-test", 0, 0.5));
        coord.receive_bid(make_bid(&mut agent_a, "job-test", 1, 0.3));
        coord.receive_bid(make_bid(&mut agent_b, "job-test", 1, 0.8));
        coord.receive_bid(make_bid(&mut agent_a, "job-test", 2, 0.4));
        coord.receive_bid(make_bid(&mut agent_b, "job-test", 2, 0.7));

        let active = vec![agent_a.id.clone(), agent_b.id.clone()];
        let assignments = coord.resolve_assignments(&active).unwrap();

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

        coord.receive_bid(make_bid(&mut agent_a, "job-test", 0, 0.5));
        coord.receive_bid(make_bid(&mut agent_b, "job-test", 0, 0.5));

        let active = vec![agent_a.id.clone(), agent_b.id.clone()];
        let assignments = coord.resolve_assignments(&active).unwrap();

        let expected = if agent_a.id < agent_b.id { &agent_a.id } else { &agent_b.id };
        assert_eq!(&assignments[&0], expected);
    }

    #[test]
    fn fallback_assigns_unbid_chunks() {
        let job = make_job(3);
        let mut coord = JobCoordinator::new(job, 2000);

        let mut agent_a = AgentIdentity::generate();
        coord.receive_bid(make_bid(&mut agent_a, "job-test", 0, 0.9));

        let active = vec![agent_a.id.clone(), "agent-b".into()];
        let assignments = coord.resolve_assignments(&active).unwrap();

        assert_eq!(assignments.len(), 3);
        assert_eq!(assignments[&0], agent_a.id);
        assert!(assignments.contains_key(&1));
        assert!(assignments.contains_key(&2));
    }

    #[test]
    fn double_resolve_returns_none() {
        let job = make_job(1);
        let mut coord = JobCoordinator::new(job, 2000);

        let mut agent = AgentIdentity::generate();
        coord.receive_bid(make_bid(&mut agent, "job-test", 0, 0.9));

        let active = vec![agent.id.clone()];
        assert!(coord.resolve_assignments(&active).is_some());
        assert!(coord.resolve_assignments(&active).is_none()); // second call blocked
    }

    #[test]
    fn accepts_poc_sigs_in_any_active_state() {
        let job = make_job(1);
        let mut coord = JobCoordinator::new(job, 2000);

        let mut agent = AgentIdentity::generate();
        coord.receive_bid(make_bid(&mut agent, "job-test", 0, 0.9));
        coord.resolve_assignments(&[agent.id.clone()]);

        // Assigned state
        coord.receive_poc_sig("agent-x".into(), "sig-1".into());
        assert_eq!(coord.poc_sigs.len(), 1);

        // BuildingPoc state
        coord.transition_to_building_poc(HashMap::new(), 1000, "hash".into());
        coord.receive_poc_sig("agent-y".into(), "sig-2".into());
        assert_eq!(coord.poc_sigs.len(), 2);

        // Dedup — same agent
        coord.receive_poc_sig("agent-x".into(), "sig-dup".into());
        assert_eq!(coord.poc_sigs.len(), 2);
    }

    #[test]
    fn deduplicates_results() {
        let job = make_job(2);
        let mut coord = JobCoordinator::new(job, 2000);
        let mut agent = AgentIdentity::generate();
        coord.receive_bid(make_bid(&mut agent, "job-test", 0, 0.9));
        coord.resolve_assignments(&[agent.id.clone()]);

        let result = ChunkResult {
            job_id: "job-test".into(),
            chunk_index: 0,
            agent_id: agent.id.clone(),
            result: "output".into(),
            result_hash: "hash".into(),
            processing_ms: 10,
            timestamp_ms: 2000,
            nonce: 0,
        };
        let env1 = agent.sign(result.clone()).unwrap();
        let env2 = agent.sign(result).unwrap();
        coord.receive_result(env1);
        coord.receive_result(env2);
        assert_eq!(coord.results.len(), 1); // deduplicated
    }
}
