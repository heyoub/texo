#!/usr/bin/env bash
# Build Texo and its isolated Linux confinement companions for the ECS host.
# Usage: ./deploy.sh <host-ip> [ssh-user]
set -euo pipefail

HOST="${1:?usage: deploy.sh <host-ip> [ssh-user]}"
SSH_USER="${2:-root}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${PORT:-8787}"

echo "==> release build"
cargo build --release --bin texo --manifest-path "$REPO_ROOT/Cargo.toml"
cargo build --release --features bvisor-helper --bin texo-bvisor-extractor \
  --manifest-path "$REPO_ROOT/Cargo.toml"
BVISOR_ROOT="$REPO_ROOT/target/bvisor-dist"
cargo install bvisor --version 0.10.0 --locked --features backend-linux \
  --bin bvisor-linux-launcher --root "$BVISOR_ROOT"

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
scp "$REPO_ROOT/target/release/texo-bvisor-extractor" \
  "${SSH_USER}@${HOST}:/opt/texo/bin/texo-bvisor-extractor"
scp "$BVISOR_ROOT/bin/bvisor-linux-launcher" \
  "${SSH_USER}@${HOST}:/opt/texo/bin/bvisor-linux-launcher"
echo "==> ship built UI (LiteShip dist — served by texo, no node on the box)"
ssh "${SSH_USER}@${HOST}" 'rm -rf /opt/texo/ui/dist && mkdir -p /opt/texo/ui'
scp -r "$REPO_ROOT/ui/dist" "${SSH_USER}@${HOST}:/opt/texo/ui/dist"
scp "$REPO_ROOT/deploy/texo-agent.service" "${SSH_USER}@${HOST}:/etc/systemd/system/"
# Never overwrite a live env file (it holds the API key).
scp "$REPO_ROOT/deploy/env.example" "${SSH_USER}@${HOST}:/opt/texo/env.example"
ssh "${SSH_USER}@${HOST}" '[ -f /opt/texo/env ] || cp /opt/texo/env.example /opt/texo/env'

echo "==> enable + start"
ssh "${SSH_USER}@${HOST}" 'systemctl daemon-reload && systemctl enable --now texo-agent && sleep 1 && systemctl --no-pager status texo-agent | head -8'

echo
echo "edit /opt/texo/env with the DashScope key, then: systemctl restart texo-agent"
echo "agent: http://${HOST}:${PORT}"
