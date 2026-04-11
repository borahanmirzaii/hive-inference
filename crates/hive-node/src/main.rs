mod ws_server;

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ed25519_dalek::VerifyingKey;
use tokio::sync::broadcast;

use hive_core::coordinator::{JobCoordinator, JobState};
use hive_core::identity::{now_ms, AgentIdentity};
use hive_core::inference;
use hive_core::logger::log;
use hive_core::network::{PeerConfig, VertexNode};
use hive_core::recovery::{redistribute_chunks, FaultDetector};
use hive_core::replay_guard::ReplayGuard;
use hive_core::types::*;

use ws_server::DashboardEvent;

#[derive(Parser)]
#[command(name = "hive-node", about = "Hive Inference — leaderless distributed AI inference")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate keypairs (Vertex ECDSA + Ed25519) for a new agent
    GenKey {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "127.0.0.1:9000")]
        addr: String,
    },
    /// Run an agent node
    Run {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        agent_name: String,
        /// Base58-encoded Vertex secret key
        #[arg(long)]
        vertex_secret: String,
        /// Hex-encoded Ed25519 secret key (32 bytes)
        #[arg(long)]
        ed25519_secret: String,
        /// Enable WebSocket dashboard server on this port (only one agent should enable)
        #[arg(long)]
        dashboard_port: Option<u16>,
    },
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct SwarmConfig {
    agent: Vec<AgentConfig>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct AgentConfig {
    name: String,
    addr: String,
    vertex_pubkey: String,
    ed25519_pubkey: String,
}

fn load_config(path: &PathBuf) -> Result<SwarmConfig> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config: {}", path.display()))?;
    toml::from_str(&content).with_context(|| "failed to parse swarm config")
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::GenKey { name, addr } => cmd_gen_key(&name, &addr),
        Command::Run {
            config,
            agent_name,
            vertex_secret,
            ed25519_secret,
            dashboard_port,
        } => cmd_run(&config, &agent_name, &vertex_secret, &ed25519_secret, dashboard_port).await,
    }
}

fn cmd_gen_key(name: &str, addr: &str) -> Result<()> {
    // Generate Vertex keypair (ECDSA prime256v1)
    let vertex_secret = tashi_vertex::KeySecret::generate();
    let vertex_pubkey = vertex_secret.public();

    // Generate Ed25519 keypair (for app-layer signing)
    let ed_identity = AgentIdentity::generate();
    let ed_secret_hex = hex::encode(ed_identity.signing_key.to_bytes());
    let ed_pubkey_hex = ed_identity.verifying_key_hex();

    println!("# Agent: {name}");
    println!("# Agent ID: {}", ed_identity.id);
    println!("#");
    println!("# Vertex secret (KEEP PRIVATE):");
    println!("# {vertex_secret}");
    println!("#");
    println!("# Ed25519 secret (KEEP PRIVATE):");
    println!("# {ed_secret_hex}");
    println!();
    println!("[[agent]]");
    println!("name = \"{name}\"");
    println!("addr = \"{addr}\"");
    println!("vertex_pubkey = \"{vertex_pubkey}\"");
    println!("ed25519_pubkey = \"{ed_pubkey_hex}\"");
    Ok(())
}

async fn cmd_run(
    config_path: &PathBuf,
    agent_name: &str,
    vertex_secret_b58: &str,
    ed25519_secret_hex: &str,
    dashboard_port: Option<u16>,
) -> Result<()> {
    let config = load_config(config_path)?;

    // Dashboard event channel
    let (event_tx, _) = broadcast::channel::<DashboardEvent>(10_000);

    // Start WebSocket dashboard server if requested
    if let Some(port) = dashboard_port {
        let addr: std::net::SocketAddr = format!("0.0.0.0:{port}").parse()?;
        let tx = event_tx.clone();
        tokio::spawn(async move {
            ws_server::run_ws_server(addr, tx).await;
        });
    }

    // Parse Vertex key
    let vertex_secret: tashi_vertex::KeySecret = vertex_secret_b58
        .parse()
        .map_err(|e: tashi_vertex::Error| anyhow::anyhow!("vertex key parse: {e}"))?;

    // Parse Ed25519 key
    let ed_bytes = hex::decode(ed25519_secret_hex)?;
    let ed_arr: [u8; 32] = ed_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("ed25519 secret must be 32 bytes"))?;
    let ed_signing = ed25519_dalek::SigningKey::from_bytes(&ed_arr);
    let mut identity = AgentIdentity::from_signing_key(ed_signing);

    // Find my entry
    let my_config = config
        .agent
        .iter()
        .find(|a| a.name == agent_name)
        .with_context(|| format!("agent '{agent_name}' not found in config"))?;
    let my_addr = my_config.addr.clone();

    // Build Vertex peer list
    let peers: Vec<PeerConfig> = config
        .agent
        .iter()
        .filter(|a| a.addr != my_addr)
        .map(|a| PeerConfig {
            addr: a.addr.clone(),
            vertex_pubkey: a.vertex_pubkey.clone(),
        })
        .collect();

    // Build Ed25519 known keys map (agent_id → VerifyingKey)
    let mut known_keys: HashMap<AgentId, VerifyingKey> = HashMap::new();
    for a in &config.agent {
        let key_bytes = hex::decode(&a.ed25519_pubkey)?;
        let arr: [u8; 32] = key_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid ed25519 pubkey for {}", a.name))?;
        let vk = VerifyingKey::from_bytes(&arr)?;
        let agent_id = hex::encode(&arr[..8]);
        known_keys.insert(agent_id, vk);
    }

    log(
        "INIT",
        agent_name,
        "STARTING",
        &format!("id={} addr={my_addr} peers={}", identity.id, peers.len()),
    );

    let node = VertexNode::start(&my_addr, &vertex_secret, &peers)
        .await
        .with_context(|| "failed to start Vertex node")?;

    log(
        "VERTEX",
        agent_name,
        "CONNECTED",
        &format!("peers={}", peers.len()),
    );
    let _ = event_tx.send(DashboardEvent::AgentConnected {
        agent_id: identity.id.clone(),
        agent_name: agent_name.to_string(),
        addr: my_addr.clone(),
        peer_count: peers.len(),
    });

    let mut replay_guard = ReplayGuard::new();
    let mut fault_detector = FaultDetector::new(5000);
    let mut jobs: HashMap<JobId, JobCoordinator> = HashMap::new();
    let mut my_load: f64 = 0.0;
    let mut poc_log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("poc_log.jsonl")?;

    fault_detector.record_heartbeat(&identity.id, now_ms());

    let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(2));
    let mut stale_check_interval = tokio::time::interval(std::time::Duration::from_secs(2));
    let mut bid_deadline_interval = tokio::time::interval(std::time::Duration::from_millis(500));

    // Only the first agent (sorted by id) submits the demo job
    let mut sorted_agent_ids: Vec<AgentId> = known_keys.keys().cloned().collect();
    sorted_agent_ids.sort();
    let i_am_first = sorted_agent_ids.first().map(|id| id == &identity.id).unwrap_or(false);

    let job_timer = tokio::time::sleep(std::time::Duration::from_secs(4));
    tokio::pin!(job_timer);
    let mut job_submitted = false;

    loop {
        tokio::select! {
            _ = heartbeat_interval.tick() => {
                let hb = Heartbeat {
                    agent_id: identity.id.clone(),
                    timestamp_ms: now_ms(),
                    nonce: 0,
                    load: my_load,
                };
                if let Ok(envelope) = identity.sign(hb) {
                    let _ = node.broadcast(&HiveMessage::Heartbeat(envelope));
                }
            }

            _ = stale_check_interval.tick() => {
                let now = now_ms();
                let stale = fault_detector.get_stale_agents(now);
                for stale_id in &stale {
                    if stale_id != &identity.id {
                        log("FAULT", agent_name, "STALE_DETECTED", &format!("agent={stale_id}"));
                        let _ = event_tx.send(DashboardEvent::StaleDetected {
                            agent_id: identity.id.clone(),
                            stale_agent: stale_id.clone(),
                        });
                    }
                }

                let alive = fault_detector.alive_agents(now);
                for (job_id, coord) in jobs.iter_mut() {
                    if let JobState::Assigned(ref assignments) = coord.state {
                        let has_stale = assignments.values().any(|a| stale.contains(a));
                        if has_stale && !stale.is_empty() {
                            let new_assignments = redistribute_chunks(assignments, &stale, &alive);
                            log("RECOVER", agent_name, "REDISTRIBUTED", &format!("job={job_id}"));
                            coord.state = JobState::Assigned(new_assignments.clone());

                            // Process newly assigned chunks
                            for (&chunk_idx, assigned_agent) in &new_assignments {
                                if assigned_agent == &identity.id {
                                    // Check if I already submitted a result for this chunk
                                    let already_done = coord.results.iter().any(|r| {
                                        r.payload.chunk_index == chunk_idx && r.payload.agent_id == identity.id
                                    });
                                    if !already_done && chunk_idx < coord.job.chunks.len() {
                                        let chunk_text = &coord.job.chunks[chunk_idx];
                                        let output = inference::process_chunk(chunk_text);
                                        my_load = (my_load + 0.2_f64).min(1.0);

                                        log("HIVE", agent_name, "CHUNK_DONE", &format!(
                                            "job={job_id} chunk={chunk_idx} ms={} hash={}...",
                                            output.processing_ms, &output.result_hash[..8]
                                        ));

                                        let result = ChunkResult {
                                            job_id: job_id.clone(),
                                            chunk_index: chunk_idx,
                                            agent_id: identity.id.clone(),
                                            result: output.result,
                                            result_hash: output.result_hash,
                                            processing_ms: output.processing_ms,
                                            timestamp_ms: now_ms(),
                                            nonce: 0,
                                        };
                                        if let Ok(env) = identity.sign(result) {
                                            let _ = node.broadcast(&HiveMessage::ChunkResult(env));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Bid deadline check — triggers assignment when deadlines expire
            _ = bid_deadline_interval.tick() => {
                let now = now_ms();
                let job_ids: Vec<JobId> = jobs.keys().cloned().collect();
                for job_id in job_ids {
                    let coord = jobs.get_mut(&job_id).unwrap();
                    if matches!(coord.state, JobState::CollectingBids)
                        && now >= coord.bid_deadline_ms
                    {
                        let mut active: Vec<AgentId> = known_keys.keys().cloned().collect();
                        active.sort();
                        let assignments = coord.resolve_assignments(&active);

                        log("HIVE", agent_name, "ASSIGNED", &format!(
                            "job={job_id} assignments={}", assignments.len()
                        ));

                        // Process my assigned chunks
                        for (&chunk_idx, assigned_agent) in &assignments {
                            if assigned_agent == &identity.id && chunk_idx < coord.job.chunks.len() {
                                let chunk_text = &coord.job.chunks[chunk_idx];
                                let output = inference::process_chunk(chunk_text);
                                my_load = (my_load + 0.2_f64).min(1.0);

                                log("HIVE", agent_name, "CHUNK_DONE", &format!(
                                    "job={job_id} chunk={chunk_idx} ms={} hash={}...",
                                    output.processing_ms, &output.result_hash[..8]
                                ));

                                let result = ChunkResult {
                                    job_id: job_id.clone(),
                                    chunk_index: chunk_idx,
                                    agent_id: identity.id.clone(),
                                    result: output.result,
                                    result_hash: output.result_hash,
                                    processing_ms: output.processing_ms,
                                    timestamp_ms: now_ms(),
                                    nonce: 0,
                                };
                                if let Ok(env) = identity.sign(result) {
                                    let _ = node.broadcast(&HiveMessage::ChunkResult(env));
                                }
                            }
                        }
                    }
                }
            }

            _ = &mut job_timer, if !job_submitted && i_am_first => {
                job_submitted = true;
                let sample_text = "The future of distributed AI inference lies in swarm coordination. \
                    Each agent processes its assigned chunk independently and efficiently. \
                    Byzantine fault tolerance ensures the system remains robust under failure. \
                    Leaderless consensus eliminates single points of failure completely. \
                    Cryptographic proofs verify that all agents coordinated correctly.";

                let chunks: Vec<String> = sample_text
                    .split(". ")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();

                let job = InferenceJob {
                    id: format!("job-{}", &identity.id[..6]),
                    submitter: identity.id.clone(),
                    chunks,
                    created_at_ms: now_ms(),
                };

                log("HIVE", agent_name, "JOB_CREATED", &format!("job={} chunks={}", job.id, job.chunks.len()));

                if let Ok(envelope) = identity.sign(job) {
                    let _ = node.broadcast(&HiveMessage::JobAnnouncement(envelope));
                }
            }

            result = node.recv() => {
                let messages = match result {
                    Ok(msgs) => msgs,
                    Err(e) => {
                        log("ERROR", agent_name, "RECV_FAILED", &format!("{e}"));
                        continue;
                    }
                };

                // Empty vec = timeout (normal), not engine close
                if messages.is_empty() {
                    continue;
                }

                for msg in messages {
                    handle_message(
                        &msg,
                        agent_name,
                        &mut identity,
                        &node,
                        &mut replay_guard,
                        &mut fault_detector,
                        &mut jobs,
                        &mut my_load,
                        &known_keys,
                        &mut poc_log,
                        &event_tx,
                    );
                }
            }
        }
    }

    #[allow(unreachable_code)]
    Ok(())
}

fn handle_message(
    msg: &HiveMessage,
    agent_name: &str,
    identity: &mut AgentIdentity,
    node: &VertexNode,
    replay_guard: &mut ReplayGuard,
    fault_detector: &mut FaultDetector,
    jobs: &mut HashMap<JobId, JobCoordinator>,
    my_load: &mut f64,
    known_keys: &HashMap<AgentId, VerifyingKey>,
    poc_log: &mut std::fs::File,
    event_tx: &broadcast::Sender<DashboardEvent>,
) {
    match msg {
        HiveMessage::Heartbeat(envelope) => {
            if let Some(vk) = known_keys.get(&envelope.signer_id) {
                if AgentIdentity::verify_envelope(envelope, vk).is_err() {
                    log("SECURITY", agent_name, "BAD_SIGNATURE", &format!("from={}", envelope.signer_id));
                    return;
                }
            } else {
                log("SECURITY", agent_name, "UNKNOWN_SIGNER", &format!("from={}", envelope.signer_id));
                return;
            }
            if let Err(e) = replay_guard.check_and_record(
                &envelope.signer_id,
                envelope.nonce,
                envelope.timestamp_ms,
            ) {
                log("REPLAY", agent_name, "REJECTED", &format!("from={} reason={e}", envelope.signer_id));
                return;
            }
            fault_detector.record_heartbeat(&envelope.payload.agent_id, envelope.payload.timestamp_ms);
            let _ = event_tx.send(DashboardEvent::Heartbeat {
                agent_id: envelope.payload.agent_id.clone(),
                load: envelope.payload.load,
                timestamp_ms: envelope.payload.timestamp_ms,
            });
        }

        HiveMessage::JobAnnouncement(envelope) => {
            if let Some(vk) = known_keys.get(&envelope.signer_id) {
                if AgentIdentity::verify_envelope(envelope, vk).is_err() {
                    log("SECURITY", agent_name, "BAD_SIG_JOB", &format!("from={}", envelope.signer_id));
                    return;
                }
            } else {
                log("SECURITY", agent_name, "UNKNOWN_SIGNER", &format!("from={}", envelope.signer_id));
                return;
            }
            if let Err(e) = replay_guard.check_and_record(
                &envelope.signer_id,
                envelope.nonce,
                envelope.timestamp_ms,
            ) {
                log("REPLAY", agent_name, "REJECTED", &format!("from={} reason={e}", envelope.signer_id));
                return;
            }

            let job = &envelope.payload;
            log("HIVE", agent_name, "JOB_RECEIVED", &format!("job={} chunks={}", job.id, job.chunks.len()));
            let _ = event_tx.send(DashboardEvent::JobCreated {
                job_id: job.id.clone(),
                submitter: job.submitter.clone(),
                chunk_count: job.chunks.len(),
                timestamp_ms: job.created_at_ms,
            });

            let coord = JobCoordinator::new(job.clone(), 2000);
            jobs.insert(job.id.clone(), coord);

            // Bid on all chunks with affinity-based scoring.
            // Each agent has a "preferred" chunk based on hash(agent_id + chunk_index).
            // This ensures deterministic distribution: different agents score higher on different chunks.
            let base_score = 1.0 / (*my_load + 1.0);
            for (i, _) in job.chunks.iter().enumerate() {
                // Affinity: hash the agent_id bytes with chunk index for deterministic variation
                let affinity_input = format!("{}{}", identity.id, i);
                let affinity_hash = hive_core::inference::sha256_hex(affinity_input.as_bytes());
                // Use first 4 hex chars as a deterministic score modifier [0.0, 1.0)
                let modifier = u16::from_str_radix(&affinity_hash[..4], 16).unwrap_or(0) as f64 / 65536.0;
                let score = base_score * (0.5 + modifier * 0.5); // range: [0.5*base, 1.0*base]

                let bid = ChunkBid {
                    job_id: job.id.clone(),
                    chunk_index: i,
                    bidder: identity.id.clone(),
                    capacity_score: score,
                    timestamp_ms: now_ms(),
                    nonce: 0,
                };
                if let Ok(env) = identity.sign(bid) {
                    let _ = node.broadcast(&HiveMessage::ChunkBid(env));
                    log("HIVE", agent_name, "BID_SENT", &format!("job={} chunk={i} score={score:.3}", job.id));
                    let _ = event_tx.send(DashboardEvent::BidSent {
                        agent_id: identity.id.clone(),
                        job_id: job.id.clone(),
                        chunk_index: i,
                        score,
                    });
                }
            }
        }

        HiveMessage::ChunkBid(envelope) => {
            if let Some(vk) = known_keys.get(&envelope.signer_id) {
                if AgentIdentity::verify_envelope(envelope, vk).is_err() {
                    return;
                }
            } else {
                return;
            }
            if replay_guard
                .check_and_record(&envelope.signer_id, envelope.nonce, envelope.timestamp_ms)
                .is_err()
            {
                return;
            }

            let job_id = &envelope.payload.job_id;
            if let Some(coord) = jobs.get_mut(job_id) {
                coord.receive_bid(envelope.clone());

                let now = now_ms();
                if now >= coord.bid_deadline_ms && matches!(coord.state, JobState::CollectingBids) {
                    let active: Vec<AgentId> = known_keys.keys().cloned().collect();
                    let assignments = coord.resolve_assignments(&active);

                    log("HIVE", agent_name, "ASSIGNED", &format!("job={job_id} assignments={}", assignments.len()));

                    // Process my assigned chunks
                    for (&chunk_idx, assigned_agent) in &assignments {
                        if assigned_agent == &identity.id && chunk_idx < coord.job.chunks.len() {
                            let chunk_text = &coord.job.chunks[chunk_idx];
                            let output = inference::process_chunk(chunk_text);
                            *my_load = (*my_load + 0.2_f64).min(1.0);

                            log("HIVE", agent_name, "CHUNK_DONE", &format!(
                                "job={job_id} chunk={chunk_idx} ms={} hash={}...",
                                output.processing_ms, &output.result_hash[..8]
                            ));

                            let result = ChunkResult {
                                job_id: job_id.clone(),
                                chunk_index: chunk_idx,
                                agent_id: identity.id.clone(),
                                result: output.result,
                                result_hash: output.result_hash,
                                processing_ms: output.processing_ms,
                                timestamp_ms: now_ms(),
                                nonce: 0,
                            };
                            if let Ok(env) = identity.sign(result) {
                                let _ = node.broadcast(&HiveMessage::ChunkResult(env));
                            }
                        }
                    }
                }
            }
        }

        HiveMessage::ChunkResult(envelope) => {
            if let Some(vk) = known_keys.get(&envelope.signer_id) {
                if AgentIdentity::verify_envelope(envelope, vk).is_err() {
                    return;
                }
            } else {
                return;
            }
            if replay_guard
                .check_and_record(&envelope.signer_id, envelope.nonce, envelope.timestamp_ms)
                .is_err()
            {
                return;
            }

            let job_id = &envelope.payload.job_id;
            if let Some(coord) = jobs.get_mut(job_id) {
                coord.receive_result(envelope.clone());

                log("HIVE", agent_name, "RESULT_RECV", &format!(
                    "job={job_id} chunk={} from={}",
                    envelope.payload.chunk_index, envelope.payload.agent_id
                ));
                let _ = event_tx.send(DashboardEvent::ResultReceived {
                    job_id: job_id.clone(),
                    chunk_index: envelope.payload.chunk_index,
                    from_agent: envelope.payload.agent_id.clone(),
                });

                // Check if all results are in
                if let JobState::Assigned(ref assignments) = coord.state {
                    if coord.results.len() >= assignments.len() {
                        // Use the job's creation timestamp — deterministic across all agents
                        let poc_timestamp = coord.job.created_at_ms;
                        let mut sorted_participants: Vec<AgentId> =
                            known_keys.keys().cloned().collect();
                        sorted_participants.sort();

                        let poc = ProofOfCoordination::build_unsigned(
                            job_id,
                            &sorted_participants,
                            assignments,
                            &coord.results,
                            poc_timestamp,
                        );

                        log("PROOF", agent_name, "POC_BUILT", &format!(
                            "job={job_id} hash={}...", &poc.poc_hash[..12]
                        ));

                        // Sign and broadcast
                        let sig = identity.sign_bytes(poc.poc_hash.as_bytes());
                        let contrib_payload = PocContributionPayload {
                            job_id: job_id.clone(),
                            agent_id: identity.id.clone(),
                            poc_hash: poc.poc_hash.clone(),
                            signature_hex: sig,
                        };
                        if let Ok(env) = identity.sign(contrib_payload) {
                            let _ = node.broadcast(&HiveMessage::PocContribution(env));
                        }

                        coord.transition_to_building_poc(
                            assignments.clone(),
                            poc_timestamp,
                            poc.poc_hash,
                        );
                    }
                }
            }
        }

        HiveMessage::PocContribution(envelope) => {
            // Verify signature
            if let Some(vk) = known_keys.get(&envelope.signer_id) {
                if AgentIdentity::verify_envelope(envelope, vk).is_err() {
                    log("REPLAY", agent_name, "BAD_SIG_POC", &format!("from={}", envelope.signer_id));
                    return;
                }
            } else {
                log("REPLAY", agent_name, "UNKNOWN_SIGNER", &format!("from={}", envelope.signer_id));
                return;
            }
            if replay_guard
                .check_and_record(&envelope.signer_id, envelope.nonce, envelope.timestamp_ms)
                .is_err()
            {
                return;
            }

            let payload = &envelope.payload;
            if let Some(coord) = jobs.get_mut(&payload.job_id) {
                coord.receive_poc_sig(payload.agent_id.clone(), payload.signature_hex.clone());

                log("PROOF", agent_name, "POC_SIG_RECV", &format!(
                    "job={} from={} sigs={}/{}",
                    payload.job_id, payload.agent_id,
                    coord.poc_sigs.len(), known_keys.len()
                ));

                let required = (known_keys.len() * 2) / 3 + 1;
                if coord.poc_sigs.len() >= required {
                    // Extract assignments and timestamp from either state
                    let (assignments, poc_timestamp) = match &coord.state {
                        JobState::BuildingPoc {
                            assignments,
                            poc_timestamp_ms,
                            ..
                        } => (assignments.clone(), *poc_timestamp_ms),
                        JobState::Assigned(a) => (a.clone(), coord.job.created_at_ms),
                        _ => return,
                    };

                    let mut sorted_participants: Vec<AgentId> =
                        known_keys.keys().cloned().collect();
                    sorted_participants.sort();

                    let mut poc = ProofOfCoordination::build_unsigned(
                        &payload.job_id,
                        &sorted_participants,
                        &assignments,
                        &coord.results,
                        poc_timestamp,
                    );
                    for (aid, sig) in &coord.poc_sigs {
                        poc.add_signature(aid.clone(), sig.clone());
                    }

                    match poc.verify(known_keys) {
                        Ok(()) => {
                            log("PROOF", agent_name, "POC_VERIFIED", &format!(
                                "job={} sigs={}/{} ✅ VERIFIED",
                                payload.job_id, poc.signatures.len(), known_keys.len()
                            ));
                            let _ = event_tx.send(DashboardEvent::PocVerified {
                                job_id: payload.job_id.clone(),
                                sig_count: poc.signatures.len(),
                                total_agents: known_keys.len(),
                            });

                            use std::io::Write;
                            if let Ok(json) = serde_json::to_string(&poc) {
                                let _ = writeln!(poc_log, "{json}");
                            }

                            coord.transition_to_complete();
                        }
                        Err(e) => {
                            log("PROOF", agent_name, "POC_FAILED", &format!(
                                "job={} error={e}", payload.job_id
                            ));
                        }
                    }
                }
            }
        }

        HiveMessage::ChunkReassignment(envelope) => {
            if let Some(vk) = known_keys.get(&envelope.signer_id) {
                if AgentIdentity::verify_envelope(envelope, vk).is_err() {
                    return;
                }
            } else {
                return;
            }

            let payload = &envelope.payload;
            if let Some(coord) = jobs.get_mut(&payload.job_id) {
                let mut new_assignments: HashMap<usize, AgentId> = HashMap::new();
                for (idx, agent) in &payload.new_assignments {
                    new_assignments.insert(*idx, agent.clone());
                }
                coord.state = JobState::Assigned(new_assignments);
                log("RECOVER", agent_name, "REASSIGNMENT", &format!(
                    "job={} stale={}", payload.job_id, payload.stale_agent
                ));
            }
        }
    }
}
