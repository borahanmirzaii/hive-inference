<script>
  import { onMount } from 'svelte';
  import { connect, onEvent } from './lib/ws.js';

  let agents = $state({});
  let logs = $state([]);
  let jobs = $state({});
  let pocVerified = $state(false);
  let pocData = $state(null);
  let connected = $state(false);
  let staleAgents = $state(new Set());

  const COLORS = {
    'agent-alpha': '#6366f1',
    'agent-beta': '#ec4899',
    'agent-gamma': '#10b981',
    'agent-delta': '#f59e0b',
    'agent-epsilon': '#06b6d4',
  };

  function getColor(agentId) {
    const idx = Object.keys(agents).sort().indexOf(agentId);
    const palette = ['#6366f1', '#ec4899', '#10b981', '#f59e0b', '#06b6d4'];
    return palette[idx % palette.length] || '#94a3b8';
  }

  function getAgentName(agentId) {
    return agents[agentId]?.name || agentId?.slice(0, 8) || '?';
  }

  function addLog(category, text, color = '#94a3b8') {
    const ts = new Date().toLocaleTimeString('en-US', { hour12: false, fractionalSecondDigits: 1 });
    logs = [{ ts, category, text, color }, ...logs].slice(0, 200);
  }

  onMount(() => {
    connect();
    const unsub = onEvent((event) => {
      switch (event.type) {
        case 'ws_connected':
          connected = true;
          addLog('SYSTEM', 'Dashboard connected to swarm', '#22c55e');
          break;
        case 'ws_disconnected':
          connected = false;
          addLog('SYSTEM', 'Disconnected from swarm', '#ef4444');
          break;
        case 'agent_connected':
          agents[event.agent_id] = {
            name: event.agent_name,
            addr: event.addr,
            peers: event.peer_count,
            load: 0,
            lastSeen: Date.now(),
            active: true,
          };
          agents = agents;
          addLog('VERTEX', `${event.agent_name} connected (${event.addr})`, getColor(event.agent_id));
          break;
        case 'heartbeat':
          if (agents[event.agent_id]) {
            agents[event.agent_id].load = event.load;
            agents[event.agent_id].lastSeen = Date.now();
            agents[event.agent_id].active = true;
            agents = agents;
          }
          staleAgents.delete(event.agent_id);
          staleAgents = staleAgents;
          break;
        case 'job_created':
          jobs[event.job_id] = {
            submitter: event.submitter,
            chunks: event.chunk_count,
            bids: [],
            results: [],
            assigned: false,
            verified: false,
          };
          jobs = jobs;
          addLog('HIVE', `Job ${event.job_id.slice(0,10)} created (${event.chunk_count} chunks)`, '#f59e0b');
          break;
        case 'bid_sent':
          if (jobs[event.job_id]) {
            jobs[event.job_id].bids.push({ agent: event.agent_id, chunk: event.chunk_index, score: event.score });
            jobs = jobs;
          }
          addLog('HIVE', `Bid: ${getAgentName(event.agent_id)} -> chunk ${event.chunk_index} (${event.score.toFixed(3)})`, getColor(event.agent_id));
          break;
        case 'chunk_assigned':
          if (jobs[event.job_id]) {
            jobs[event.job_id].assigned = true;
            jobs = jobs;
          }
          addLog('HIVE', `Assigned: chunk ${event.chunk_index} -> ${getAgentName(event.agent_id)}`, getColor(event.agent_id));
          break;
        case 'chunk_done':
          addLog('HIVE', `Done: ${getAgentName(event.agent_id)} chunk ${event.chunk_index} (${event.processing_ms}ms)`, getColor(event.agent_id));
          break;
        case 'result_received':
          if (jobs[event.job_id]) {
            jobs[event.job_id].results.push({ chunk: event.chunk_index, from: event.from_agent });
            jobs = jobs;
          }
          addLog('HIVE', `Result: chunk ${event.chunk_index} from ${getAgentName(event.from_agent)}`, getColor(event.from_agent));
          break;
        case 'poc_built':
          addLog('PROOF', `PoC built: ${event.poc_hash?.slice(0, 16)}...`, '#a855f7');
          break;
        case 'poc_sig_received':
          addLog('PROOF', `Signature ${event.sig_count}/${event.total_agents} from ${getAgentName(event.from_agent)}`, '#a855f7');
          break;
        case 'poc_verified':
          pocVerified = true;
          pocData = event;
          if (jobs[event.job_id]) {
            jobs[event.job_id].verified = true;
            jobs = jobs;
          }
          addLog('PROOF', `VERIFIED ${event.sig_count}/${event.total_agents} signatures`, '#22c55e');
          break;
        case 'stale_detected':
          staleAgents.add(event.stale_agent);
          staleAgents = staleAgents;
          if (agents[event.stale_agent]) {
            agents[event.stale_agent].active = false;
            agents = agents;
          }
          addLog('FAULT', `STALE: ${getAgentName(event.stale_agent)} unresponsive`, '#ef4444');
          break;
        case 'redistributed':
          addLog('RECOVER', `Chunks redistributed for job ${event.job_id?.slice(0, 10)}`, '#f59e0b');
          break;
        case 'replay_rejected':
          addLog('SECURITY', `Replay REJECTED from ${getAgentName(event.from_agent)}: ${event.reason}`, '#ef4444');
          break;
      }
    });
    return unsub;
  });

  // Agent positions for mesh visualization (pentagon layout)
  function getAgentPos(index, total) {
    const angle = (index / total) * Math.PI * 2 - Math.PI / 2;
    const r = 120;
    return { x: 160 + r * Math.cos(angle), y: 160 + r * Math.sin(angle) };
  }
</script>

<div class="dashboard">
  <header>
    <div class="title">
      <h1>HIVE INFERENCE</h1>
      <span class="subtitle">Leaderless Distributed AI Inference on Tashi Vertex</span>
    </div>
    <div class="status">
      <span class="dot" class:online={connected} class:offline={!connected}></span>
      {connected ? 'LIVE' : 'OFFLINE'}
    </div>
  </header>

  <div class="grid">
    <!-- Mesh View -->
    <div class="panel mesh-panel">
      <h2>Agent Mesh</h2>
      <svg viewBox="0 0 320 320" class="mesh-svg">
        <!-- Connection lines -->
        {#each Object.keys(agents).sort() as aid1, i}
          {#each Object.keys(agents).sort().slice(i + 1) as aid2, j}
            {@const p1 = getAgentPos(i, Object.keys(agents).length)}
            {@const p2 = getAgentPos(Object.keys(agents).sort().indexOf(aid2), Object.keys(agents).length)}
            <line
              x1={p1.x} y1={p1.y}
              x2={p2.x} y2={p2.y}
              stroke={staleAgents.has(aid1) || staleAgents.has(aid2) ? '#374151' : '#4b5563'}
              stroke-width="1"
              opacity="0.5"
            />
          {/each}
        {/each}

        <!-- Agent nodes -->
        {#each Object.entries(agents).sort(([a], [b]) => a.localeCompare(b)) as [id, agent], i}
          {@const pos = getAgentPos(i, Object.keys(agents).length)}
          {@const isStale = staleAgents.has(id)}

          <!-- Pulse ring -->
          {#if agent.active && !isStale}
            <circle
              cx={pos.x} cy={pos.y}
              r="24"
              fill="none"
              stroke={getColor(id)}
              stroke-width="1"
              opacity="0.3"
              class="pulse"
            />
          {/if}

          <!-- Node circle -->
          <circle
            cx={pos.x} cy={pos.y}
            r="18"
            fill={isStale ? '#374151' : getColor(id)}
            opacity={isStale ? 0.4 : 1}
            stroke={isStale ? '#ef4444' : 'none'}
            stroke-width={isStale ? 2 : 0}
          />

          <!-- Agent label -->
          <text
            x={pos.x} y={pos.y + 1}
            text-anchor="middle"
            dominant-baseline="middle"
            fill="white"
            font-size="8"
            font-weight="600"
            font-family="JetBrains Mono"
          >{agent.name?.replace('agent-', '')[0]?.toUpperCase()}</text>

          <!-- Name below -->
          <text
            x={pos.x} y={pos.y + 34}
            text-anchor="middle"
            fill={isStale ? '#6b7280' : '#d1d5db'}
            font-size="9"
            font-family="Inter"
          >{agent.name?.replace('agent-', '')}</text>

          <!-- Load bar -->
          <rect x={pos.x - 14} y={pos.y + 40} width="28" height="3" rx="1" fill="#1f2937" />
          <rect x={pos.x - 14} y={pos.y + 40} width={28 * agent.load} height="3" rx="1" fill={getColor(id)} opacity="0.8" />

          {#if isStale}
            <text x={pos.x} y={pos.y + 55} text-anchor="middle" fill="#ef4444" font-size="8" font-weight="700">STALE</text>
          {/if}
        {/each}
      </svg>
    </div>

    <!-- Job Progress -->
    <div class="panel job-panel">
      <h2>Jobs</h2>
      {#if Object.keys(jobs).length === 0}
        <p class="muted">Waiting for jobs...</p>
      {/if}
      {#each Object.entries(jobs) as [jobId, job]}
        <div class="job-card" class:verified={job.verified}>
          <div class="job-header">
            <span class="job-id">{jobId.slice(0, 12)}</span>
            {#if job.verified}
              <span class="badge verified-badge">VERIFIED</span>
            {:else if job.assigned}
              <span class="badge assigned-badge">PROCESSING</span>
            {:else}
              <span class="badge">BIDDING</span>
            {/if}
          </div>
          <div class="chunks">
            {#each Array(job.chunks) as _, i}
              {@const hasResult = job.results.some(r => r.chunk === i)}
              <div
                class="chunk"
                class:done={hasResult}
                style:background-color={hasResult ? '#22c55e' : '#374151'}
              >
                {i}
              </div>
            {/each}
          </div>
          <div class="job-meta">
            Bids: {job.bids.length} | Results: {job.results.length}/{job.chunks}
          </div>
        </div>
      {/each}

      {#if pocVerified && pocData}
        <div class="poc-card">
          <div class="poc-header">Proof of Coordination</div>
          <div class="poc-detail">Signatures: {pocData.sig_count}/{pocData.total_agents}</div>
          <div class="poc-detail">Status: <span class="poc-ok">VERIFIED</span></div>
        </div>
      {/if}
    </div>

    <!-- Log Panel -->
    <div class="panel log-panel">
      <h2>Live Events</h2>
      <div class="log-scroll">
        {#each logs as entry}
          <div class="log-line">
            <span class="log-ts">{entry.ts}</span>
            <span class="log-cat" style:color={entry.color}>[{entry.category}]</span>
            <span class="log-text">{entry.text}</span>
          </div>
        {/each}
      </div>
    </div>
  </div>
</div>

<style>
  :global(body) {
    margin: 0;
    padding: 0;
    background: #0f172a;
    color: #e2e8f0;
    font-family: 'Inter', sans-serif;
    overflow: hidden;
    height: 100vh;
  }

  .dashboard {
    display: flex;
    flex-direction: column;
    height: 100vh;
    padding: 16px;
    box-sizing: border-box;
    gap: 16px;
  }

  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0 8px;
  }

  h1 {
    margin: 0;
    font-family: 'JetBrains Mono', monospace;
    font-size: 24px;
    font-weight: 700;
    background: linear-gradient(135deg, #6366f1, #ec4899);
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    letter-spacing: 2px;
  }

  .subtitle {
    font-size: 12px;
    color: #64748b;
    display: block;
    margin-top: 2px;
  }

  .status {
    display: flex;
    align-items: center;
    gap: 8px;
    font-family: 'JetBrains Mono', monospace;
    font-size: 13px;
    font-weight: 600;
  }

  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
  }

  .online { background: #22c55e; box-shadow: 0 0 8px #22c55e; }
  .offline { background: #ef4444; box-shadow: 0 0 8px #ef4444; }

  .grid {
    display: grid;
    grid-template-columns: 340px 1fr 1fr;
    gap: 16px;
    flex: 1;
    min-height: 0;
  }

  .panel {
    background: #1e293b;
    border-radius: 12px;
    padding: 16px;
    border: 1px solid #334155;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  h2 {
    margin: 0 0 12px 0;
    font-size: 13px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 1px;
    color: #94a3b8;
  }

  .mesh-svg {
    width: 100%;
    flex: 1;
  }

  .pulse {
    animation: pulse-ring 2s ease-in-out infinite;
  }

  @keyframes pulse-ring {
    0%, 100% { r: 20; opacity: 0; }
    50% { r: 28; opacity: 0.4; }
  }

  .muted {
    color: #475569;
    font-size: 13px;
    text-align: center;
    padding: 20px;
  }

  .job-card {
    background: #0f172a;
    border-radius: 8px;
    padding: 12px;
    margin-bottom: 8px;
    border: 1px solid #334155;
    transition: border-color 0.3s;
  }

  .job-card.verified {
    border-color: #22c55e;
    box-shadow: 0 0 12px rgba(34, 197, 94, 0.15);
  }

  .job-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 8px;
  }

  .job-id {
    font-family: 'JetBrains Mono', monospace;
    font-size: 12px;
    color: #94a3b8;
  }

  .badge {
    font-size: 10px;
    font-weight: 700;
    padding: 2px 8px;
    border-radius: 4px;
    background: #374151;
    color: #9ca3af;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .assigned-badge { background: #1d4ed8; color: #93c5fd; }
  .verified-badge { background: #166534; color: #86efac; animation: glow 1s ease-in-out infinite alternate; }

  @keyframes glow {
    from { box-shadow: 0 0 4px #22c55e; }
    to { box-shadow: 0 0 12px #22c55e; }
  }

  .chunks {
    display: flex;
    gap: 4px;
    margin-bottom: 6px;
  }

  .chunk {
    width: 28px;
    height: 28px;
    border-radius: 4px;
    display: flex;
    align-items: center;
    justify-content: center;
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
    font-weight: 600;
    color: white;
    transition: background-color 0.3s;
  }

  .chunk.done { box-shadow: 0 0 6px rgba(34, 197, 94, 0.3); }

  .job-meta {
    font-size: 11px;
    color: #64748b;
    font-family: 'JetBrains Mono', monospace;
  }

  .poc-card {
    background: linear-gradient(135deg, #064e3b, #166534);
    border-radius: 8px;
    padding: 12px;
    margin-top: 12px;
    border: 1px solid #22c55e;
    animation: glow 1.5s ease-in-out infinite alternate;
  }

  .poc-header {
    font-weight: 700;
    font-size: 13px;
    margin-bottom: 6px;
    color: #86efac;
  }

  .poc-detail {
    font-size: 12px;
    color: #a7f3d0;
    font-family: 'JetBrains Mono', monospace;
  }

  .poc-ok {
    color: #22c55e;
    font-weight: 700;
  }

  .log-panel {
    min-height: 0;
  }

  .log-scroll {
    flex: 1;
    overflow-y: auto;
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
    line-height: 1.6;
  }

  .log-scroll::-webkit-scrollbar { width: 4px; }
  .log-scroll::-webkit-scrollbar-track { background: transparent; }
  .log-scroll::-webkit-scrollbar-thumb { background: #334155; border-radius: 2px; }

  .log-line {
    display: flex;
    gap: 8px;
    padding: 1px 0;
    animation: fadeIn 0.2s ease-out;
  }

  @keyframes fadeIn {
    from { opacity: 0; transform: translateX(-4px); }
    to { opacity: 1; transform: translateX(0); }
  }

  .log-ts { color: #475569; min-width: 80px; }
  .log-cat { min-width: 70px; font-weight: 600; }
  .log-text { color: #cbd5e1; }
</style>
