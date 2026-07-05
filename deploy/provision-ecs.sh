#!/usr/bin/env bash
# Provision the texo-agent backend host on Alibaba Cloud ECS.
#
# Uses the Alibaba Cloud CLI (`aliyun`) — https://github.com/aliyun/aliyun-cli —
# authenticated via `aliyun configure` (AccessKey for the hackathon account).
# Region defaults to ap-southeast-1 (Singapore), the same side as the
# DashScope international endpoint the agent calls.
#
# Idempotence: this script creates named resources and is safe to re-run only
# after deleting them; it is a hackathon provisioning script, not IaC.
set -euo pipefail

REGION="${REGION:-ap-southeast-1}"
ZONE="${ZONE:-ap-southeast-1a}"
INSTANCE_TYPE="${INSTANCE_TYPE:-ecs.e-c1m2.large}"   # 2 vCPU / 4 GiB, burstable
IMAGE_ID="${IMAGE_ID:-ubuntu_24_04_x64_20G_alibase_20250722.vhd}"
KEY_PAIR="${KEY_PAIR:?set KEY_PAIR to an existing ECS key pair name}"
SG_NAME="${SG_NAME:-texo-agent-sg}"
INSTANCE_NAME="${INSTANCE_NAME:-texo-agent}"
AGENT_PORT="${AGENT_PORT:-8787}"

echo "==> security group (${SG_NAME}) in ${REGION}"
SG_ID=$(aliyun ecs CreateSecurityGroup --RegionId "$REGION" \
  --SecurityGroupName "$SG_NAME" \
  --Description "texo-agent hackathon backend" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["SecurityGroupId"])')
echo "    ${SG_ID}"

echo "==> ingress rules (ssh 22, agent ${AGENT_PORT})"
for PORT in 22 "$AGENT_PORT"; do
  aliyun ecs AuthorizeSecurityGroup --RegionId "$REGION" \
    --SecurityGroupId "$SG_ID" --IpProtocol tcp \
    --PortRange "${PORT}/${PORT}" --SourceCidrIp 0.0.0.0/0 >/dev/null
done

echo "==> ECS instance (${INSTANCE_TYPE}, ${IMAGE_ID})"
INSTANCE_ID=$(aliyun ecs RunInstances --RegionId "$REGION" --ZoneId "$ZONE" \
  --InstanceType "$INSTANCE_TYPE" --ImageId "$IMAGE_ID" \
  --SecurityGroupId "$SG_ID" --KeyPairName "$KEY_PAIR" \
  --InstanceName "$INSTANCE_NAME" \
  --InternetMaxBandwidthOut 5 \
  --SystemDisk.Category cloud_essd --SystemDisk.Size 40 \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["InstanceIdSets"]["InstanceIdSet"][0])')
echo "    ${INSTANCE_ID}"

echo "==> waiting for public IP"
for _ in $(seq 1 30); do
  IP=$(aliyun ecs DescribeInstances --RegionId "$REGION" \
    --InstanceIds "[\"$INSTANCE_ID\"]" \
    | python3 -c 'import json,sys
d=json.load(sys.stdin)["Instances"]["Instance"]
ips=d[0]["PublicIpAddress"]["IpAddress"] if d else []
print(ips[0] if ips else "")')
  [ -n "$IP" ] && break
  sleep 5
done
[ -n "${IP:-}" ] || { echo "no public IP after 150s"; exit 1; }

echo
echo "instance: ${INSTANCE_ID}"
echo "public ip: ${IP}"
echo "next: ./deploy.sh ${IP}"
