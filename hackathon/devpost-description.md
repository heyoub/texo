# texo-agent — memory that knows when to stop believing things

**Track 1: MemoryAgent**

<!-- TODO: video link -->

texo-agent is a chat agent whose persistent memory is an append-only claim-chain, not a vector store — memory that knows when to stop believing things. Facts you state across sessions become journaled claims; when a fact changes, the old claim is retired with a receipt recording *what* superseded it, *when*, and from *which source line*. The agent doesn't just remember — it can tell you the history of what it used to believe and why it stopped.

## The problem with accumulate-only memory

Most agent memory is RAG over a vector database: every fact ever stated gets embedded, and retrieval hopes the right one ranks first. When a preference changes — "deploys moved to Tuesday" — the Friday fact doesn't go away. It sits in the index forever, semantically near-identical to its replacement, waiting to be retrieved into a context window. Accumulate-only memory has no concept of *outdated*; it only has *less similar*. Contradictions aren't resolved, they're ranked.

## How texo-agent works

Every chat turn is journaled the moment it lands — sessions live in hidden per-session lanes of the same append-only journal, so a mid-session crash loses nothing and there is no separate session store to drift out of sync: the journal *is* the session. On session end, the lane is rendered to markdown (user utterances only) and an LLM extraction pass turns it into atomic claims, appended to a hash-committed journal (built on BatPak, an event-sourcing log). A relate pass then embeds the current claims, clusters them, and puts candidate pairs in front of an LLM relation judge that decides: supersede, conflict, duplicate, or unrelated. A supersession retires the old claim with a receipt; a genuine disagreement is surfaced as an open conflict rather than silently resolved in favor of the newer statement.

At the start of every turn, the journal is replayed into a projection: current claims are injected into the system prompt as trusted memory, superseded claims are injected as "outdated — do NOT trust" with what replaced them, and open conflicts are listed so the model presents both sides instead of picking one. Every claim carries full provenance: source `path:line`, span-level byte offsets into the transcript, and the extractor model + prompt version that produced it. Ask the agent what it remembers and it cites its receipts.

<!-- TODO: architecture diagram -->

## What makes it technically deep

The journal is append-only and hash-committed, and replay is deterministic: the model runs exactly once, at ingest (a record-once perception boundary), its outputs cached content-addressed by `model | prompt_version | span` — re-ingesting a corpus went from 16 s to 4 ms, byte-identical. A deterministic faithfulness gate (integer parts-per-million token recall — journaled state contains no floats, so projections stay `Eq` and replay bit-for-bit) rejects extracted claims not grounded in the source text. The relation judge is cluster-bounded: candidate pairs are limited to connected components of the cosine-similarity graph, so judge calls scale with cluster size rather than O(n²) over the whole memory. And supersession requires explicit change wording ("moved to", "no longer"); a bare differing value is recorded as a conflict, because a memory system that silently trusts the newer sentence is just a slower way to be wrong.

The whole thing is one Rust binary. Every surface — CLI, HTTP, MCP — dispatches through the same operation kit, and every journal write verifies its append receipt inline before the operation is allowed to succeed. The HTTP server, the SSE stream, the HTTPS client that talks to DashScope, and the MCP transport are hand-rolled sync code on the substrate's own primitives; there is no tokio, no axum, no reqwest in the tree. And the repo audits itself: `just drift` feeds texo's own documentation back through the pipeline, and the old architecture's claims surface as superseded — with receipts — by the docs that replaced them.

## Built on Qwen Cloud

All three LLM roles — claim extraction, the relation judge, and chat — are wired to Qwen models on Qwen Cloud's DashScope OpenAI-compatible mode (`dashscope-intl`, Singapore); the cosine prefilter uses `text-embedding-v4`. The agent backend targets an Alibaba Cloud ECS instance in the same Singapore region (`ap-southeast-1`) as the model endpoint; the deployment is fully scripted and committed to the repo (`deploy/provision-ecs.sh`, `deploy/deploy.sh`), with provisioning pending.
<!-- TODO: switch to present tense ("runs on <model-id>", "is deployed on ECS ...") once the instance is provisioned and a live Qwen Cloud run is verified end-to-end -->

## Pre-existing project disclosure

The texo library (claim-chain journal, replay, CLI, MCP server) predates the submission window; the rules allow this when the project is significantly updated in-window, so here is exactly what was built during the window: **the agent itself** (`texo serve` — the chat loop, crash-safe session lanes, session-end memorization pipeline, memory-grounded prompting, and its real-store test suite); **a ground-up architecture rebuild** (six crates flattened to one crate and one binary; tokio/axum/reqwest/rmcp replaced by hand-rolled sync HTTP, SSE, and MCP on the substrate's own primitives — 492 locked dependencies down to 324 — with every claim ID proven identical across the rewrite, so the rebuilt system demonstrably remembers everything the old one did); the **BatPak 0.9.0 migration**, behavior-preserving with an empirical store-compatibility proof (a 0.8.2-written store replays byte-identically under 0.9.0); **char-offset + model provenance** on every recorded claim (span byte ranges, extractor model, prompt version); **cluster-first relate**, the O(n²)→O(n·cluster) fix live-validated at 5/5 on the regression corpus; the **live UI** (LiteShip: chat with per-claim citation chips, a memory sidebar and journal timeline fed by the journal's own event stream, a receipts panel that cross-checks the binary's interface fingerprint between the stream handshake and the host endpoint, and the drift view); and the licensing/documentation compliance sweep. The full dated changelog is in `HACKATHON.md`.

## Track 1 requirement mapping

| Track 1 asks for | Mechanism |
|---|---|
| Persistent memory accumulating across sessions | append-only BatPak journal; session-end ingest; deterministic replay |
| Timely forgetting of outdated information | supersession events with receipts — typed forgetting, not decay heuristics |
| Recalling critical memories in limited context | replayed *current-claims* projection injected per turn; superseded claims quarantined |
| Efficient storage & retrieval | content-addressed claims, embedding prefilter, cluster-bounded judging, record-once LLM cache |
