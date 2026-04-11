use std::collections::HashMap;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::types::*;

impl ProofOfCoordination {
    pub fn build_unsigned(
        job_id: &str,
        participants: &[AgentId],
        assignments: &HashMap<ChunkIndex, AgentId>,
        results: &[SignedEnvelope<ChunkResult>],
        consensus_timestamp_ms: u64,
    ) -> Self {
        let mut sorted_participants = participants.to_vec();
        sorted_participants.sort();

        let mut sorted_assignments: Vec<(AgentId, ChunkIndex)> = assignments
            .iter()
            .map(|(idx, agent)| (agent.clone(), *idx))
            .collect();
        sorted_assignments.sort_by_key(|(_, idx)| *idx);

        let mut sorted_result_hashes: Vec<(AgentId, String)> = results
            .iter()
            .map(|r| {
                (
                    r.payload.agent_id.clone(),
                    r.payload.result_hash.clone(),
                )
            })
            .collect();
        sorted_result_hashes.sort_by(|a, b| a.0.cmp(&b.0));

        let poc_hash =
            Self::compute_hash(job_id, &sorted_participants, &sorted_assignments, &sorted_result_hashes, consensus_timestamp_ms);

        ProofOfCoordination {
            job_id: job_id.to_string(),
            participants: sorted_participants,
            chunk_assignments: sorted_assignments,
            result_hashes: sorted_result_hashes,
            consensus_timestamp_ms,
            poc_hash,
            signatures: vec![],
        }
    }

    pub fn add_signature(&mut self, agent_id: AgentId, signature_hex: String) {
        self.signatures.push((agent_id, signature_hex));
        self.signatures.sort_by(|a, b| a.0.cmp(&b.0));
    }

    pub fn verify(&self, known_keys: &HashMap<AgentId, VerifyingKey>) -> Result<(), String> {
        // 1. Recompute hash
        let expected_hash = Self::compute_hash(
            &self.job_id,
            &self.participants,
            &self.chunk_assignments,
            &self.result_hashes,
            self.consensus_timestamp_ms,
        );

        if expected_hash != self.poc_hash {
            return Err(format!(
                "PoC hash mismatch: expected {expected_hash}, got {}",
                self.poc_hash
            ));
        }

        // 2. Verify each signature
        let mut valid_sigs = 0usize;
        for (agent_id, sig_hex) in &self.signatures {
            let Some(vk) = known_keys.get(agent_id) else {
                continue;
            };
            let Ok(sig_bytes) = hex::decode(sig_hex) else {
                continue;
            };
            let Ok(sig_arr): Result<[u8; 64], _> = sig_bytes.try_into() else {
                continue;
            };
            let sig = Signature::from_bytes(&sig_arr);
            if vk.verify(self.poc_hash.as_bytes(), &sig).is_ok() {
                valid_sigs += 1;
            }
        }

        // 3. Supermajority: >2/3
        let required = (self.participants.len() * 2) / 3 + 1;
        if valid_sigs < required {
            return Err(format!(
                "insufficient signatures: {valid_sigs}/{} (need {required})",
                self.participants.len()
            ));
        }

        Ok(())
    }

    fn compute_hash(
        job_id: &str,
        participants: &[AgentId],
        assignments: &[(AgentId, ChunkIndex)],
        result_hashes: &[(AgentId, String)],
        consensus_timestamp_ms: u64,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(job_id.as_bytes());
        for p in participants {
            hasher.update(p.as_bytes());
        }
        for (agent, idx) in assignments {
            hasher.update(agent.as_bytes());
            hasher.update(idx.to_le_bytes());
        }
        for (agent, hash) in result_hashes {
            hasher.update(agent.as_bytes());
            hasher.update(hash.as_bytes());
        }
        hasher.update(consensus_timestamp_ms.to_le_bytes());
        hex::encode(hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::AgentIdentity;
    use ed25519_dalek::Signer;

    #[test]
    fn build_and_verify_poc() {
        let mut agents: Vec<AgentIdentity> = (0..5).map(|_| AgentIdentity::generate()).collect();

        let participants: Vec<AgentId> = agents.iter().map(|a| a.id.clone()).collect();

        let mut assignments = HashMap::new();
        for (i, agent) in agents.iter().enumerate() {
            assignments.insert(i, agent.id.clone());
        }

        // Mock results
        let results: Vec<SignedEnvelope<ChunkResult>> = agents
            .iter_mut()
            .enumerate()
            .map(|(i, agent)| {
                let cr = ChunkResult {
                    job_id: "job-001".into(),
                    chunk_index: i,
                    agent_id: agent.id.clone(),
                    result: format!("result-{i}"),
                    result_hash: format!("hash-{i}"),
                    processing_ms: 10,
                    timestamp_ms: 1000,
                    nonce: 0,
                };
                agent.sign(cr).unwrap()
            })
            .collect();

        let mut poc =
            ProofOfCoordination::build_unsigned("job-001", &participants, &assignments, &results, 1000);

        // Each agent signs the poc_hash
        for agent in &agents {
            let sig = agent.signing_key.sign(poc.poc_hash.as_bytes());
            poc.add_signature(agent.id.clone(), hex::encode(sig.to_bytes()));
        }

        // Verify
        let known_keys: HashMap<AgentId, VerifyingKey> = agents
            .iter()
            .map(|a| (a.id.clone(), a.verifying_key))
            .collect();

        poc.verify(&known_keys).unwrap();
    }

    #[test]
    fn reject_insufficient_signatures() {
        let agents: Vec<AgentIdentity> = (0..5).map(|_| AgentIdentity::generate()).collect();
        let participants: Vec<AgentId> = agents.iter().map(|a| a.id.clone()).collect();
        let assignments = HashMap::new();
        let results = vec![];

        let mut poc =
            ProofOfCoordination::build_unsigned("job-002", &participants, &assignments, &results, 2000);

        // Only 1 signature (need 4 for 5 participants)
        let sig = agents[0].signing_key.sign(poc.poc_hash.as_bytes());
        poc.add_signature(agents[0].id.clone(), hex::encode(sig.to_bytes()));

        let known_keys: HashMap<AgentId, VerifyingKey> = agents
            .iter()
            .map(|a| (a.id.clone(), a.verifying_key))
            .collect();

        assert!(poc.verify(&known_keys).is_err());
    }
}
