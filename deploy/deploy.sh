#!/usr/bin/env bash
# Build texo-agent + texo-extract and deploy them to the ECS host as a
# systemd service. Usage: ./deploy.sh <host-ip> [ssh-user]
set -euo pipefail

HOST="${1:?usage: deploy.sh <host-ip> [ssh-user]}"
SSH_USER="${2:-root}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "==> release build"
cargo build --release -p texo-agent -p texo-extract \
  --manifest-path "$REPO_ROOT/Cargo.toml"

echo "==> ship binaries + unit + env template"
ssh "${SSH_USER}@${HOST}" 'mkdir -p /opt/texo-agent/bin /opt/texo-agent/workspace'
scp "$REPO_ROOT/target/release/texo-agent" \
    "$REPO_ROOT/target/release/texo-extract" \
    "${SSH_USER}@${HOST}:/opt/texo-agent/bin/"
scp "$REPO_ROOT/deploy/texo-agent.service" "${SSH_USER}@${HOST}:/etc/systemd/system/"
# Never overwrite a live env file (it holds the API key).
scp "$REPO_ROOT/deploy/env.example" "${SSH_USER}@${HOST}:/opt/texo-agent/env.example"
ssh "${SSH_USER}@${HOST}" '[ -f /opt/texo-agent/env ] || cp /opt/texo-agent/env.example /opt/texo-agent/env'

echo "==> enable + start"
ssh "${SSH_USER}@${HOST}" 'systemctl daemon-reload && systemctl enable --now texo-agent && sleep 1 && systemctl --no-pager status texo-agent | head -8'

echo
echo "edit /opt/texo-agent/env with the DashScope key, then: systemctl restart texo-agent"
echo "agent: http://${HOST}:8787"
