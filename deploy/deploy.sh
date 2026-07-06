#!/usr/bin/env bash
# Build texo and deploy the single binary to the ECS host as a systemd service.
# Usage: ./deploy.sh <host-ip> [ssh-user]
set -euo pipefail

HOST="${1:?usage: deploy.sh <host-ip> [ssh-user]}"
SSH_USER="${2:-root}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "==> release build"
cargo build --release --bin texo --manifest-path "$REPO_ROOT/Cargo.toml"

echo "==> ship binary + unit + env template"
ssh "${SSH_USER}@${HOST}" '
  set -euo pipefail
  systemctl stop texo-agent || true
  mkdir -p /opt/texo/bin /opt/texo
  if [ -d /opt/texo-agent/workspace ] && [ ! -e /opt/texo/workspace ]; then
    mv /opt/texo-agent/workspace /opt/texo/workspace
  else
    mkdir -p /opt/texo/workspace
  fi
  if [ -f /opt/texo-agent/env ] && [ ! -f /opt/texo/env ]; then
    mv /opt/texo-agent/env /opt/texo/env
  fi
'
scp "$REPO_ROOT/target/release/texo" "${SSH_USER}@${HOST}:/opt/texo/bin/texo"
scp "$REPO_ROOT/deploy/texo-agent.service" "${SSH_USER}@${HOST}:/etc/systemd/system/"
# Never overwrite a live env file (it holds the API key).
scp "$REPO_ROOT/deploy/env.example" "${SSH_USER}@${HOST}:/opt/texo/env.example"
ssh "${SSH_USER}@${HOST}" '[ -f /opt/texo/env ] || cp /opt/texo/env.example /opt/texo/env'

echo "==> enable + start"
ssh "${SSH_USER}@${HOST}" 'systemctl daemon-reload && systemctl enable --now texo-agent && sleep 1 && systemctl --no-pager status texo-agent | head -8'

echo
echo "edit /opt/texo/env with the DashScope key, then: systemctl restart texo-agent"
echo "agent: http://${HOST}:8787"
