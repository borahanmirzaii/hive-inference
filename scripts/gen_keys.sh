#!/usr/bin/env bash
# Generate 5 agent keypairs and write config/swarm.toml
# Run from project root: bash scripts/gen_keys.sh
set -euo pipefail

echo "Building hive-node..."
cargo build --bin hive-node 2>&1

BINARY="./target/debug/hive-node"

# Find the dylib directory
DYLIB_DIR=$(find ./target/debug/build -name "libtashi-vertex.dylib" -path "*/out/lib/*" -exec dirname {} \; | head -1)
if [ -z "$DYLIB_DIR" ]; then
    echo "Error: libtashi-vertex.dylib not found. Build may have failed."
    exit 1
fi
export DYLD_LIBRARY_PATH="$DYLIB_DIR"

mkdir -p config

AGENTS=("agent-alpha" "agent-beta" "agent-gamma" "agent-delta" "agent-epsilon")
PORTS=(9000 9001 9002 9003 9004)

SWARM_TOML="config/swarm.toml"
SECRETS_FILE="config/secrets.env"

echo "# Hive Inference Swarm Config" > "$SWARM_TOML"
echo "" >> "$SWARM_TOML"

echo "# KEEP THIS FILE PRIVATE — contains secret keys" > "$SECRETS_FILE"
echo "# Source before running: source config/secrets.env" >> "$SECRETS_FILE"
echo "" >> "$SECRETS_FILE"

for i in "${!AGENTS[@]}"; do
    NAME="${AGENTS[$i]}"
    ADDR="127.0.0.1:${PORTS[$i]}"

    echo "Generating keys for $NAME ($ADDR)..."

    OUTPUT=$("$BINARY" gen-key --name "$NAME" --addr "$ADDR" 2>&1)

    VERTEX_SECRET=$(echo "$OUTPUT" | grep "^# Vertex secret" -A1 | tail -1 | sed 's/^# //')
    ED25519_SECRET=$(echo "$OUTPUT" | grep "^# Ed25519 secret" -A1 | tail -1 | sed 's/^# //')
    VERTEX_PUBKEY=$(echo "$OUTPUT" | grep "vertex_pubkey" | sed 's/.*= "//' | sed 's/"//')
    ED25519_PUBKEY=$(echo "$OUTPUT" | grep "ed25519_pubkey" | sed 's/.*= "//' | sed 's/"//')
    AGENT_ID=$(echo "$OUTPUT" | grep "^# Agent ID:" | sed 's/.*: //')

    cat >> "$SWARM_TOML" << EOF
[[agent]]
name = "$NAME"
addr = "$ADDR"
vertex_pubkey = "$VERTEX_PUBKEY"
ed25519_pubkey = "$ED25519_PUBKEY"

EOF

    ENV_NAME=$(echo "$NAME" | tr '-' '_' | tr '[:lower:]' '[:upper:]')
    cat >> "$SECRETS_FILE" << EOF
# $NAME (id: $AGENT_ID)
${ENV_NAME}_VERTEX_SECRET="$VERTEX_SECRET"
${ENV_NAME}_ED25519_SECRET="$ED25519_SECRET"

EOF

    echo "  done: $NAME (id: $AGENT_ID)"
done

echo ""
echo "Config: $SWARM_TOML"
echo "Secrets: $SECRETS_FILE"
echo ""
echo "Next: source config/secrets.env && bash scripts/run_swarm.sh"
