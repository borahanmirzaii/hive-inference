use std::collections::{BTreeSet, HashMap};
use thiserror::Error;

use crate::types::AgentId;

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("message too old: {age_ms}ms")]
    TooOld { age_ms: u64 },
    #[error("duplicate nonce: {nonce}")]
    DuplicateNonce { nonce: u64 },
    #[error("stale nonce {nonce} (max seen: {max})")]
    StaleNonce { nonce: u64, max: u64 },
}

pub struct ReplayGuard {
    seen_nonces: HashMap<AgentId, BTreeSet<u64>>,
    timestamp_tolerance_ms: u64,
    nonce_window: u64,
}

impl ReplayGuard {
    pub fn new() -> Self {
        Self {
            seen_nonces: HashMap::new(),
            timestamp_tolerance_ms: 30_000,
            nonce_window: 1000,
        }
    }

    pub fn check_and_record(
        &mut self,
        agent_id: &AgentId,
        nonce: u64,
        timestamp_ms: u64,
    ) -> Result<(), ReplayError> {
        let now = crate::identity::now_ms();

        if now.saturating_sub(timestamp_ms) > self.timestamp_tolerance_ms {
            return Err(ReplayError::TooOld {
                age_ms: now - timestamp_ms,
            });
        }

        let nonces = self.seen_nonces.entry(agent_id.clone()).or_default();

        if nonces.contains(&nonce) {
            return Err(ReplayError::DuplicateNonce { nonce });
        }

        if let Some(&max_nonce) = nonces.iter().next_back() {
            if max_nonce > self.nonce_window && nonce < max_nonce - self.nonce_window {
                return Err(ReplayError::StaleNonce {
                    nonce,
                    max: max_nonce,
                });
            }
        }

        nonces.insert(nonce);

        // Prune to bound memory
        if nonces.len() > 2000 {
            let cutoff = nonces.iter().nth(1000).copied().unwrap_or(0);
            nonces.retain(|&n| n >= cutoff);
        }

        Ok(())
    }
}

impl Default for ReplayGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_duplicate_nonce() {
        let mut guard = ReplayGuard::new();
        let agent = "test-agent".to_string();
        let now = crate::identity::now_ms();
        guard.check_and_record(&agent, 1, now).unwrap();
        assert!(guard.check_and_record(&agent, 1, now).is_err());
    }

    #[test]
    fn accepts_fresh_nonces() {
        let mut guard = ReplayGuard::new();
        let agent = "test-agent".to_string();
        let now = crate::identity::now_ms();
        guard.check_and_record(&agent, 1, now).unwrap();
        guard.check_and_record(&agent, 2, now).unwrap();
        guard.check_and_record(&agent, 3, now).unwrap();
    }
}
