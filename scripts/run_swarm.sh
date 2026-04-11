#!/usr/bin/env bash
# Launch 5 agents in tmux panes
# Run from project root: bash scripts/run_swarm.sh
set -euo pipefail

CONFIG="config/swarm.toml"
SECRETS="config/secrets.env"

if [ ! -f "$CONFIG" ]; then
    echo "Error: $CONFIG not found. Run: bash scripts/gen_keys.sh"
    exit 1
fi
if [ ! -f "$SECRETS" ]; then
    echo "Error: $SECRETS not found. Run: bash scripts/gen_keys.sh"
    exit 1
fi

source "$SECRETS"

echo "Building hive-node..."
cargo build --bin hive-node 2>&1

BINARY="$(pwd)/target/debug/hive-node"

# Find the dylib directory
DYLIB_DIR=$(find ./target/debug/build -name "libtashi-vertex.dylib" -path "*/out/lib/*" -exec dirname {} \; | head -1)
if [ -z "$DYLIB_DIR" ]; then
    echo "Error: libtashi-vertex.dylib not found."
    exit 1
fi
DYLIB_DIR="$(cd "$DYLIB_DIR" && pwd)"
CONFIG_ABS="$(pwd)/$CONFIG"

SESSION="hive-swarm"

# Kill existing session
tmux kill-session -t "$SESSION" 2>/dev/null || true

# Clean old PoC log
rm -f poc_log.jsonl

# Helper: build the run command for an agent
run_cmd() {
    local NAME="$1"
    local VS="$2"
    local ES="$3"
    local EXTRA="${4:-}"
    echo "cd $(pwd) && DYLD_LIBRARY_PATH=$DYLIB_DIR $BINARY run --config $CONFIG_ABS --agent-name $NAME --vertex-secret $VS --ed25519-secret $ES $EXTRA; echo '--- $NAME exited ---'; read"
}

# Alpha also runs the dashboard WebSocket server on port 3001
tmux new-session -d -s "$SESSION" -n "swarm" \
    "$(run_cmd agent-alpha "$AGENT_ALPHA_VERTEX_SECRET" "$AGENT_ALPHA_ED25519_SECRET" "--dashboard-port 3001")"

tmux split-window -t "$SESSION" -h \
    "$(run_cmd agent-beta "$AGENT_BETA_VERTEX_SECRET" "$AGENT_BETA_ED25519_SECRET")"

tmux split-window -t "$SESSION" -v \
    "$(run_cmd agent-gamma "$AGENT_GAMMA_VERTEX_SECRET" "$AGENT_GAMMA_ED25519_SECRET")"

tmux select-pane -t 0
tmux split-window -t "$SESSION" -v \
    "$(run_cmd agent-delta "$AGENT_DELTA_VERTEX_SECRET" "$AGENT_DELTA_ED25519_SECRET")"

tmux split-window -t "$SESSION" -v \
    "$(run_cmd agent-epsilon "$AGENT_EPSILON_VERTEX_SECRET" "$AGENT_EPSILON_ED25519_SECRET")"

tmux select-layout -t "$SESSION" tiled

echo ""
echo "Swarm launched in tmux session: $SESSION"
echo "Attach with: tmux attach -t $SESSION"
echo "Kill with:   tmux kill-session -t $SESSION"
echo ""
echo "Run demo:    bash scripts/demo.sh"
