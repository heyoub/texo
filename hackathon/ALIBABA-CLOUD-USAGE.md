# Alibaba Cloud API usage — judge reference

One-hop map of where this project calls Alibaba Cloud, for submission review.

## 1. Qwen models via Model Studio (DashScope OpenAI-compatible mode)

Every model call goes through one HTTP client
([`src/surfaces/openai.rs`](../src/surfaces/openai.rs)) whose base URL and
models are configuration, resolved by the AI gateway
([`src/gateway.rs`](../src/gateway.rs)). The agent's gateway is configured for
DashScope compatible mode:

```
TEXO_LLM_BASE_URL=https://dashscope-intl.aliyuncs.com/compatible-mode/v1
TEXO_LLM_CHAT_MODEL=qwen3.7-max        # agent chat        (POST /chat/completions)
TEXO_LLM_PROPOSE_MODEL=qwen3.7-max     # claim extraction  (POST /chat/completions)
TEXO_LLM_RELATE_MODEL=qwen3.7-max      # semantic judge    (POST /chat/completions)
TEXO_LLM_EMBED_MODEL=text-embedding-v4 # candidate embed   (POST /embeddings)
```

See [`deploy/env.example`](../deploy/env.example) for the full deploy
configuration (key redacted).

## 2. ECS / VPC provisioning (Alibaba Cloud OpenAPI via `aliyun` CLI)

[`deploy/provision-ecs.sh`](../deploy/provision-ecs.sh) provisions the
backend host using these ECS/VPC API operations:

| Operation | Purpose |
|---|---|
| `CreateVpc` / `CreateVSwitch` | isolated network for the agent host |
| `CreateSecurityGroup` / `AuthorizeSecurityGroup` | SSH (restricted CIDR) + agent port ingress |
| `RunInstances` | the ECS instance (Ubuntu 24.04) that runs `texo serve` |
| `DescribeInstances` | resolve the public IP for deploy + health checks |

[`deploy/deploy.sh`](../deploy/deploy.sh) ships the release binary and UI to
the instance; [`deploy/texo-agent.service`](../deploy/texo-agent.service)
runs it under systemd.

## 3. Deployment proof

[`scripts/proof-ecs.sh`](../scripts/proof-ecs.sh) is the deployment-proof
pass to run once the instance is live: `/api/health` on the ECS host, a
three-session memory arc (teach → supersede → fresh-session recall with
receipts), and the redacted host environment showing the DashScope endpoint
and Qwen model configuration.
