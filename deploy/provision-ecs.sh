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
IMAGE_ID="${IMAGE_ID:-ubuntu_24_04_x64_20G_alibase_20260615.vhd}"
KEY_PAIR="${KEY_PAIR:?set KEY_PAIR to an existing ECS key pair name}"
SG_NAME="${SG_NAME:-texo-agent-sg}"
INSTANCE_NAME="${INSTANCE_NAME:-texo-agent}"
VPC_NAME="${VPC_NAME:-texo-vpc}"
VSWITCH_NAME="${VSWITCH_NAME:-texo-vsw}"
VPC_CIDR="${VPC_CIDR:-172.16.0.0/16}"
VSWITCH_CIDR="${VSWITCH_CIDR:-172.16.0.0/24}"
PORT="${PORT:-8787}"
SSH_INGRESS_CIDR="${SSH_INGRESS_CIDR:?set SSH_INGRESS_CIDR to your public IP in CIDR form}"
AGENT_INGRESS_CIDR="${AGENT_INGRESS_CIDR:-0.0.0.0/0}"

# The region requires VPC networking (classic instances are gone:
# OperationDenied.InvalidNetworkType). Create the VPC + vswitch first and
# scope the security group to the VPC.
echo "==> vpc + vswitch in ${REGION}/${ZONE}"
VPC_ID=$(aliyun vpc CreateVpc --RegionId "$REGION" --CidrBlock "$VPC_CIDR" \
  --VpcName "$VPC_NAME" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["VpcId"])')
sleep 5
VSW_ID=$(aliyun vpc CreateVSwitch --RegionId "$REGION" --ZoneId "$ZONE" \
  --VpcId "$VPC_ID" --CidrBlock "$VSWITCH_CIDR" --VSwitchName "$VSWITCH_NAME" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["VSwitchId"])')
echo "    ${VPC_ID} / ${VSW_ID}"

echo "==> security group (${SG_NAME}) in ${REGION}"
SG_ID=$(aliyun ecs CreateSecurityGroup --RegionId "$REGION" \
  --VpcId "$VPC_ID" \
  --SecurityGroupName "$SG_NAME" \
  --Description "texo-agent hackathon backend" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["SecurityGroupId"])')
echo "    ${SG_ID}"

echo "==> ingress rules (ssh 22 from ${SSH_INGRESS_CIDR}, agent ${PORT} from ${AGENT_INGRESS_CIDR})"
aliyun ecs AuthorizeSecurityGroup --RegionId "$REGION" \
  --SecurityGroupId "$SG_ID" --IpProtocol tcp \
  --PortRange "22/22" --SourceCidrIp "$SSH_INGRESS_CIDR" >/dev/null
aliyun ecs AuthorizeSecurityGroup --RegionId "$REGION" \
  --SecurityGroupId "$SG_ID" --IpProtocol tcp \
  --PortRange "${PORT}/${PORT}" --SourceCidrIp "$AGENT_INGRESS_CIDR" >/dev/null

echo "==> ECS instance (${INSTANCE_TYPE}, ${IMAGE_ID})"
INSTANCE_ID=$(aliyun ecs RunInstances --RegionId "$REGION" --ZoneId "$ZONE" \
  --InstanceType "$INSTANCE_TYPE" --ImageId "$IMAGE_ID" \
  --SecurityGroupId "$SG_ID" --VSwitchId "$VSW_ID" --KeyPairName "$KEY_PAIR" \
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
