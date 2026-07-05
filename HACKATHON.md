# Qwen Cloud hackathon — Track 1: MemoryAgent

Working doc for the [Global AI Hackathon Series with Qwen Cloud](https://qwencloud-hackathon.devpost.com/)
submission. **Deadline: Jul 9, 2026, 2:00pm PT.**

## The pitch

Every memory agent accumulates; almost none *retire*. RAG-over-a-vector-DB
memory keeps contradictory preferences forever and hopes retrieval ranks the
right one. texo's claim-chain gives an agent memory with **supersession
semantics**: when a remembered fact changes, the old claim is retired with a
receipt — *what* superseded it, *when*, from *which source line*. The agent
doesn't just remember; it knows when to stop believing things.

Track-1 requirement → texo mechanism:

| Track 1 asks for | texo mechanism |
|---|---|
| Persistent memory accumulating across sessions | append-only BatPak journal, deterministic replay |
| Timely forgetting of outdated information | supersession events + stale-claim projection (typed forgetting, not decay) |
| Recalling critical memories in limited context | compiled current-context projection (JSON/MCP), only *current* claims |
| Efficient storage & retrieval | content-addressed claims, embed prefilter, record-once LLM cache |

## Submission requirements → status

- [x] Public repo (`github.com/heyoub/texo`)
- [x] Open-source license, detectable in About (LICENSE-MIT + LICENSE-APACHE, matches Cargo.toml)
- [ ] Uses Qwen models on Qwen Cloud (DashScope OpenAI-compatible mode — see gaps below)
- [x] The agent itself (Track 1 wants an *agent*, not a library — `crates/texo-agent`, see below)
- [ ] Backend deployed on Alibaba Cloud + short proof recording + judge-visible code file using Alibaba Cloud APIs
- [ ] Architecture diagram (Qwen Cloud ↔ backend ↔ journal ↔ agent surfaces)
- [ ] ~3-min public demo video (YouTube/Vimeo)
- [ ] Text description + track identification on Devpost
- [ ] Optional: blog post (separate $500×10 prize)

**Rules note — pre-existing project.** texo predates the submission window
(opened May 26, 2026). The rules allow this if the project is *significantly
updated* during the window and the update is explained. Everything below the
line in "In-window changelog" is built during the window; the Devpost
description must say so explicitly.

## Gaps to close (code)

1. ~~**Embed model has no override.**~~ **Done (Jul 4).** `OPENROUTER_EMBED_MODEL`
   and `OPENROUTER_RERANK_MODEL` now wired through the same explicit → env →
   default chain as the other roles.
2. **Validate the pipeline on Qwen models.** Extractor + relater prompts
   demand strict JSON; verify with the config below. Oracle: the key-gated
   live tests + the Helios corpus (must stay 5/5).
3. ~~**The memory agent.**~~ **Done pending Qwen validation (Jul 5).**
   `crates/texo-agent` — an axum HTTP chat agent as a new thin consumer of
   texo (sibling to the MCP surface — texo stays the substrate, journal stays
   source of truth):
   - every turn injects the replayed *current* claims as trusted memory (with
     `path:line` + byte-span receipts) and lists superseded claims as
     "outdated — do not trust";
   - `POST /api/session/end` → transcript rendered to `sessions/<id>.md` →
     ingest (`texo-extract` LLM path via `extractor_cmd`) → relate;
   - changed preferences get *superseded*, and the one-file UI at `/` shows
     the chain live (current, struck-through stale + what replaced it,
     conflicts). Chat model via `OPENROUTER_CHAT_MODEL`; remaining work is
     validating the whole loop on Qwen models (gap 2).
4. **Alibaba Cloud deployment.** Agent backend on ECS; deploy config/script in
   repo doubles as the judge-visible "uses Alibaba Cloud" code file.
5. **Env-var naming (optional, optics).** The seam is generic OpenAI-compatible
   but the vars are `OPENROUTER_*`. Consider neutral aliases (`TEXO_LLM_*`)
   so the Qwen configuration doesn't read as another vendor's. Docs updated
   either way. (SPEC.md "Surfaces" gains the agent when it lands.)

Deliberately **not** in scope: ADR-002 code-awareness and the WASM roadmap
item (both post-submission). The batpak 0.8.2 → 0.9.0 bump — originally
deferred for deadline risk — landed Jul 4 after a behavior-preserving
migration with empirical store-compat proof (a 0.8.2-written store replays
byte-identically on 0.9.0; full suite + frozen Helios regression green, zero
golden churn). No 0.9.0 primitives (lanes, `import_events`) are adopted yet.

## Qwen Cloud setup (verified Jul 4, 2026)

- **Console / signup:** [home.qwencloud.com](https://home.qwencloud.com) — free
  90-day quota on signup, no payment method required. API keys at
  `home.qwencloud.com/api-keys`. Hackathon credits coupon: form linked from the
  [Devpost resources page](https://qwencloud-hackathon.devpost.com/resources).
- **Endpoint:** OpenAI-compatible base URL (international / Singapore):
  `https://dashscope-intl.aliyuncs.com/compatible-mode/v1` — serves both
  `/chat/completions` and `/embeddings`. Keys are **not** interchangeable
  across regions.
- **Models:** chat `qwen3.7-max` (flagship) / `qwen3.7-plus`; embeddings
  `text-embedding-v4` (works through compatible-mode `/embeddings`,
  `dimensions` param supported). `qwen3-rerank` exists but only via
  DashScope's native API — irrelevant here: texo's relate path uses only
  embedder + relater.

```sh
export OPENROUTER_BASE_URL=https://dashscope-intl.aliyuncs.com/compatible-mode/v1
export OPENROUTER_API_KEY=sk-...              # DashScope/Qwen Cloud key
export OPENROUTER_EXTRACTOR_MODEL=qwen3.7-max
export OPENROUTER_RELATER_MODEL=qwen3.7-max
export OPENROUTER_EMBED_MODEL=text-embedding-v4
```

(`qwen3.7-plus` for both LLM roles once validated, if cost matters; the
relater is the hardest judgment in the pipeline, so downgrade it last.)

- **Deployment (Alibaba Cloud proper, separate account/console at
  [alibabacloud.com](https://www.alibabacloud.com)):** ECS instance in
  Singapore (`ap-southeast-1`, same side as `dashscope-intl`), agent backend as
  a systemd service. The deploy script committed to this repo doubles as the
  judge-visible "uses Alibaba Cloud" code file; record the proof video on the
  live instance. Check the "Alibaba Resource Guide" + "Proof of Deployment"
  Drive docs on the Devpost resources page before provisioning.

## Plan

- **Jul 4** — docs/compliance sweep (this commit); register on Devpost, claim
  Qwen Cloud trial + hackathon credits coupon.
- **Jul 5** — embed-model override; Helios green on Qwen models end-to-end.
- **Jul 5–6** — memory agent loop (chat + ingest-on-session-end + context
  compile), cross-session demo scenario.
- **Jul 7** — ECS deployment, proof recording, architecture diagram.
- **Jul 8** — demo video, Devpost description, blog post. **Submit Jul 8**,
  one day early.

## Known demo consideration (transcript phrasing)

Chat transcripts are **first-person prose**. The extractor tends to normalize
subjects ("we deploy on Fridays" → "The team deploys on Fridays"), and the
faithfulness gate — correctly — rejects paraphrases whose entities aren't
grounded in the source, so a first-person fact can fail to journal. Observed
live Jul 5 (the claim was proposed, then gated). Declaratively-phrased facts
journal reliably; the demo script should phrase facts with explicit subjects.
Proper fix is extractor-prompt iteration against a transcript-style oracle
case (same discipline as the reverted self-assertion rule) — post-submission
unless time allows.

## In-window changelog (the "significantly updated" evidence)

- **Jul 4:** license files added; docs generalized to OpenAI-compatible
  backends; this plan.
- **Jul 4:** env overrides for the embedder and reranker models
  (`OPENROUTER_EMBED_MODEL`, `OPENROUTER_RERANK_MODEL`) — the last two roles
  whose models could only be set programmatically; model-precedence logic
  factored pure (`pick_model`) and unit-tested.
- **Jul 4:** proposer self-assertion rule attempted (`PROPOSE_PROMPT_VERSION`
  3→4) and **reverted the same day** after the live integrated oracle dropped
  to 4/5 — see ROADMAP for the lesson (prompt changes are pipeline changes).
- **Jul 4:** VS Code extension manners landed (timeout, debounce, status bar,
  missing-config notice).
- **Jul 4:** batpak 0.8.2 → 0.9.0 migration landed, behavior-preserving, with
  empirical store-compat proof (0.8.2-written store replays byte-identically).
- **Jul 4:** char offsets + model provenance on `ClaimRecorded` landed
  (ROADMAP v1.1 item): span-level byte ranges + extractor model/prompt
  version on every claim, `#[serde(default)]` back-compat, zero golden churn,
  claim IDs unchanged — precise "jump to source" for the memory agent.
- **Jul 4:** cluster-first relate landed (O(n²) fix): judge pairs bounded to
  within connected-component clusters (~O(n · cluster_size)), verdict
  semantics unchanged, live-validated 5/5 on Helios. Default
  `cosine_threshold` retuned 0.78 → 0.65 (measured Helios same-subject floor
  ≈ 0.70; rationale at the constant). Directly serves the memory agent:
  repeated relate passes over a growing memory corpus stay affordable.
- **Jul 5:** `texo-agent` landed — the Track-1 memory agent (axum server +
  one-file vanilla-JS UI). Chat is grounded in the replayed current claims
  (every injected memory carries `path:line` + byte-span receipts;
  superseded claims are injected as "outdated — do not trust"); session end
  renders the transcript to `sessions/<id>.md`, ingests through the
  `texo-extract` seam, and runs the relate pass so the next session sees the
  updated chain. Self-bootstraps its workspace (`extractor_cmd` +
  `[semantics] enabled`), honors the record-once cache env vars, keeps
  BatPak I/O on `spawn_blocking` behind texo-core APIs. Real-store tests
  only (cross-session supersession, span receipts slicing the transcript,
  HTTP surface in-process); the LLM call is isolated behind a unit-tested
  pure request builder.
- **Jul 5:** memory agent driven live end-to-end (three-session demo: teach →
  change → recall; supersession with receipts and cited recall all verified
  against real models). Transcript ingestion narrowed to user utterances only
  — the assistant's restatements were journaling every fact twice.
