use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use thiserror::Error;

use crate::types::{AgentId, SignedEnvelope};

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("hex decode error: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("invalid signature length")]
    BadSignatureLength,
    #[error("signature verification failed: {0}")]
    Verification(#[from] ed25519_dalek::SignatureError),
}

pub struct AgentIdentity {
    pub id: AgentId,
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
    nonce_counter: u64,
}

impl AgentIdentity {
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let id = hex::encode(&verifying_key.to_bytes()[..8]);
        Self {
            id,
            signing_key,
            verifying_key,
            nonce_counter: 0,
        }
    }

    pub fn from_signing_key(signing_key: SigningKey) -> Self {
        let verifying_key = signing_key.verifying_key();
        let id = hex::encode(&verifying_key.to_bytes()[..8]);
        Self {
            id,
            signing_key,
            verifying_key,
            nonce_counter: 0,
        }
    }

    pub fn verifying_key_hex(&self) -> String {
        hex::encode(self.verifying_key.to_bytes())
    }

    pub fn sign<T: serde::Serialize>(
        &mut self,
        payload: T,
    ) -> Result<SignedEnvelope<T>, IdentityError> {
        let nonce = self.next_nonce();
        let timestamp_ms = now_ms();
        let payload_json = serde_json::to_string(&payload)?;
        let to_sign = format!("{}{}{}", payload_json, nonce, timestamp_ms);
        let sig: Signature = self.signing_key.sign(to_sign.as_bytes());
        Ok(SignedEnvelope {
            payload,
            signer_id: self.id.clone(),
            nonce,
            timestamp_ms,
            signature: hex::encode(sig.to_bytes()),
        })
    }

    pub fn sign_bytes(&self, data: &[u8]) -> String {
        let sig: Signature = self.signing_key.sign(data);
        hex::encode(sig.to_bytes())
    }

    pub fn verify_envelope<T: serde::Serialize>(
        envelope: &SignedEnvelope<T>,
        verifying_key: &VerifyingKey,
    ) -> Result<(), IdentityError> {
        let payload_json = serde_json::to_string(&envelope.payload)?;
        let to_verify = format!(
            "{}{}{}",
            payload_json, envelope.nonce, envelope.timestamp_ms
        );
        let sig_bytes =
            hex::decode(&envelope.signature).map_err(IdentityError::Hex)?;
        let sig_arr: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| IdentityError::BadSignatureLength)?;
        let sig = Signature::from_bytes(&sig_arr);
        verifying_key
            .verify(to_verify.as_bytes(), &sig)
            .map_err(IdentityError::Verification)?;
        Ok(())
    }

    fn next_nonce(&mut self) -> u64 {
        self.nonce_counter += 1;
        self.nonce_counter
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Heartbeat;

    #[test]
    fn sign_and_verify() {
        let mut agent = AgentIdentity::generate();
        let hb = Heartbeat {
            agent_id: agent.id.clone(),
            timestamp_ms: now_ms(),
            nonce: 0,
            load: 0.5,
        };
        let envelope = agent.sign(hb).unwrap();
        AgentIdentity::verify_envelope(&envelope, &agent.verifying_key).unwrap();
    }

    #[test]
    fn reject_tampered_payload() {
        let mut agent = AgentIdentity::generate();
        let hb = Heartbeat {
            agent_id: agent.id.clone(),
            timestamp_ms: now_ms(),
            nonce: 0,
            load: 0.5,
        };
        let mut envelope = agent.sign(hb).unwrap();
        envelope.payload.load = 0.9; // tamper
        assert!(
            AgentIdentity::verify_envelope(&envelope, &agent.verifying_key).is_err()
        );
    }
}
