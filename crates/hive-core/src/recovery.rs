use std::collections::HashMap;

use crate::types::{AgentId, ChunkIndex};

pub struct FaultDetector {
    last_heartbeat: HashMap<AgentId, u64>,
    stale_threshold_ms: u64,
}

impl FaultDetector {
    pub fn new(stale_threshold_ms: u64) -> Self {
        Self {
            last_heartbeat: HashMap::new(),
            stale_threshold_ms,
        }
    }

    pub fn record_heartbeat(&mut self, agent_id: &AgentId, timestamp_ms: u64) {
        self.last_heartbeat.insert(agent_id.clone(), timestamp_ms);
    }

    pub fn get_stale_agents(&self, now_ms: u64) -> Vec<AgentId> {
        let mut stale: Vec<AgentId> = self
            .last_heartbeat
            .iter()
            .filter(|(_, &last)| now_ms.saturating_sub(last) > self.stale_threshold_ms)
            .map(|(id, _)| id.clone())
            .collect();
        stale.sort(); // deterministic ordering
        stale
    }

    pub fn is_alive(&self, agent_id: &AgentId, now_ms: u64) -> bool {
        self.last_heartbeat
            .get(agent_id)
            .map(|&last| now_ms.saturating_sub(last) <= self.stale_threshold_ms)
            .unwrap_or(false)
    }

    pub fn alive_agents(&self, now_ms: u64) -> Vec<AgentId> {
        let mut alive: Vec<AgentId> = self
            .last_heartbeat
            .iter()
            .filter(|(_, &last)| now_ms.saturating_sub(last) <= self.stale_threshold_ms)
            .map(|(id, _)| id.clone())
            .collect();
        alive.sort(); // deterministic
        alive
    }
}

/// Deterministic redistribution of orphaned chunks from stale agents.
/// Every surviving node runs this and gets the same result.
pub fn redistribute_chunks(
    assignments: &HashMap<ChunkIndex, AgentId>,
    stale_agents: &[AgentId],
    alive_agents: &[AgentId],
) -> HashMap<ChunkIndex, AgentId> {
    let mut new_assignments = assignments.clone();
    let mut sorted_alive = alive_agents.to_vec();
    sorted_alive.sort();

    // Collect orphaned chunks in deterministic order
    let mut orphaned: Vec<ChunkIndex> = assignments
        .iter()
        .filter(|(_, agent)| stale_agents.contains(agent))
        .map(|(&idx, _)| idx)
        .collect();
    orphaned.sort();

    for (i, chunk_idx) in orphaned.into_iter().enumerate() {
        if sorted_alive.is_empty() {
            break;
        }
        let new_agent = &sorted_alive[i % sorted_alive.len()];
        new_assignments.insert(chunk_idx, new_agent.clone());
    }

    new_assignments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redistributes_orphaned_chunks() {
        let mut assignments = HashMap::new();
        assignments.insert(0, "agent-a".to_string());
        assignments.insert(1, "agent-b".to_string());
        assignments.insert(2, "agent-c".to_string());

        let stale = vec!["agent-b".to_string()];
        let alive = vec!["agent-a".to_string(), "agent-c".to_string()];

        let new = redistribute_chunks(&assignments, &stale, &alive);
        assert_eq!(new[&0], "agent-a");
        assert_ne!(new[&1], "agent-b"); // redistributed
        assert_eq!(new[&2], "agent-c");
    }

    #[test]
    fn detects_stale_agents() {
        let mut fd = FaultDetector::new(5000);
        fd.record_heartbeat(&"agent-a".to_string(), 1000);
        fd.record_heartbeat(&"agent-b".to_string(), 1000);

        let stale = fd.get_stale_agents(7000);
        assert_eq!(stale.len(), 2); // both stale at 7000

        fd.record_heartbeat(&"agent-a".to_string(), 6000);
        let stale = fd.get_stale_agents(7000);
        assert_eq!(stale.len(), 1); // only b is stale
        assert_eq!(stale[0], "agent-b");
    }
}
