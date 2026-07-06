# Deploying texo to Alibaba Cloud

This directory is the submission's **proof-of-Alibaba-Cloud code**: the
backend is provisioned with the [Alibaba Cloud CLI](https://github.com/aliyun/aliyun-cli)
(ECS APIs: `CreateSecurityGroup`, `AuthorizeSecurityGroup`, `RunInstances`,
`DescribeInstances`) and runs on an ECS instance in `ap-southeast-1`
(Singapore — same region as the DashScope endpoint the agent calls).

```sh
aliyun configure                       # AccessKey auth, region ap-southeast-1
KEY_PAIR=<your-ecs-keypair> ./provision-ecs.sh
./deploy.sh <public-ip>                # build, ship, systemd enable --now
ssh root@<public-ip> 'vi /opt/texo/env && systemctl restart texo-agent'
```

- `provision-ecs.sh` — security group + ECS instance via Alibaba Cloud APIs
- `deploy.sh` — release build, scp the single `texo` binary, install + start systemd unit
- `texo-agent.service` — systemd unit (journal workspace under `/opt/texo`)
- `env.example` — Qwen Cloud (DashScope compatible-mode) model configuration

## Proof-recording checklist (per hackathon rules)

Record a short screen capture, separate from the demo video, showing:

1. The Alibaba Cloud ECS console with the running `texo-agent` instance
   (region + instance id visible).
2. `ssh` into the instance: `systemctl status texo-agent` active, and
   `curl -s localhost:8787/api/memory | head` answering.
3. The browser UI at `http://<public-ip>:8787` doing one chat turn.
4. This directory in the public repo (the judge-visible code-file link).

> Status: scripts authored Jul 5; to be executed against the live account on
> deployment day (Jul 7 per HACKATHON.md plan).
