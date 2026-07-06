# Teaching an agent to stop believing things

*Five days building a memory agent on my own event-sourcing substrate, for the Qwen Cloud hackathon.*

I build two tools that turned out to be one tool. BatPak is a content-addressed, append-only event log — event sourcing with receipts. texo sits on top of it: a "claim-chain" that ingests prose, extracts atomic claims, and journals them as typed events, so that when a fact changes, the old claim is *superseded* — retired with a receipt saying what replaced it, when, and from which source line. The pitch I've been repeating for months is "git tracks code diffs; texo tracks claim diffs."

When the Qwen Cloud hackathon published its tracks, Track 1 — MemoryAgent — read like a spec I'd already implemented by accident. It asks for memory that accumulates across sessions, *forgets outdated information in a timely way*, and recalls what matters into a limited context. Every memory agent I've seen accumulates; almost none retire. RAG-over-a-vector-DB memory keeps your contradictory preferences forever and hopes retrieval ranks the right one. Supersession semantics is the missing verb. The catch: texo was a library with a CLI and an MCP server. Track 1 wants an *agent*. I had five days, July 4 to July 8, to build one.

<!-- TODO: architecture diagram -->

## Day one: the substrate settles

July 4 was plumbing with two surprises. The boring parts: license files, backend-neutral docs, and env overrides for the two model roles (embedder, reranker) that could previously only be set programmatically — which mattered, because the defaults pointed at models that don't exist on DashScope-compatible hosts, and the whole submission runs on Qwen Cloud's compatible mode.

The first surprise was BatPak itself. I'd deliberately deferred texo's 0.8.2 → 0.9.0 bump as deadline risk — migrating your event store's storage engine mid-hackathon is how demos die. Then 0.9.0 shipped mid-window and the migration turned out to be mechanical: a receipt-verification API rename, a field rename, one `#[must_use]`. What made it safe to land anyway was empirical, not vibes: I took a store written by the 0.8.2 build, opened it under 0.9.0, and verified it replayed byte-identically — same claims JSON, same agent context, receipts green — then appended new events into the old store and re-verified. Zero golden-test churn. When your journal is the product, "behavior-preserving" is a measurement, not a claim.

The second surprise was the one I'm happiest to write about, because it went wrong.

## The revert is the best part

texo's extractor prompt has a versioned constant, `PROPOSE_PROMPT_VERSION`, because the record-once cache is keyed by `model | prompt_version | span` — change the prompt, invalidate the cache. I had a hardening item queued: the extractor faithfully records claims a document makes about *itself* ("this wiki is the source of truth for new engineers") as durable facts, which is ironic for a tool whose thesis is "prose is not state." So I added one exclusion bullet to the prompt, bumped v3 → v4, and ran the tests.

The live integrated oracle — the messy seven-file "Helios" corpus that has to score 5/5 end-to-end against real models — dropped to 4/5. The Postgres→BatPak supersession, which had been rock solid, degraded into an unresolved conflict. One bullet point, aimed at self-assertions, had shifted extraction *wording* broadly enough that a claim's embedding moved, the cosine clustering (freshly rebuilt that same day to bound the relation judge to within-cluster pairs) split a true pair across clusters, and the judge never saw them together. Wording → embedding → cluster split → lost supersession.

I reverted the same day and wrote the lesson into the roadmap: **prompt changes are pipeline changes.** They get iterated against the live oracle before landing, ideally with a new oracle case for the behavior you're targeting — not eyeballed as "just a prompt tweak." A frozen regression suite wouldn't have caught this; only running the real models did. Keeping a live oracle in the loop cost me API pennies and saved me from shipping a memory agent that had quietly forgotten how to forget.

## Day two: the agent, and what live driving taught it

The agent itself (at that point `texo-agent`, an axum HTTP server with a one-file UI — hold that thought) took shape fast, because it's a thin consumer of the substrate: every chat turn replays the journal and injects *current* claims into the system prompt as trusted memory — each one carrying `path:line` plus span-level byte offsets — while superseded claims are injected under a header that says exactly what I want the model to internalize: `## Outdated memory (do NOT trust — superseded)`. Ending a session renders the transcript to `sessions/<id>.md`, ingests it through the LLM extractor, and runs the relate pass so the next session wakes up with the updated chain.

Then I drove it live — three sessions: teach, change, recall — and reality filed two bugs.

First: **the assistant was double-journaling everything.** A helpful chat model restates your facts back at you ("Got it — deploys are on Tuesdays now"), and my transcript renderer was ingesting both speakers. Every fact journaled twice, once per speaker, as distinct current claims. The fix was a one-line philosophy decision: transcripts render *user* utterances only. Memory is what the user said, not the model's paraphrase of it.

Second: **first-person phrasing collided with the faithfulness gate.** People talk in first person — "we deploy on Fridays." The extractor tends to normalize subjects, proposing "The team deploys on Fridays," and texo's faithfulness gate — a deterministic token-recall check, computed in integer parts-per-million because journaled state contains no floats — correctly rejected the paraphrase: "The team" isn't grounded in the source text. I watched the claim get proposed and then gated, live. The gate was right, which is the interesting part; the honest fix is extractor-prompt iteration against a transcript-style oracle case — the exact same discipline as the reverted self-assertion rule — so it's documented and deferred rather than hacked in at midnight. The demo phrases facts with explicit subjects.

## What supersession-with-receipts feels like

Session one, I teach it facts. Session two, I change one: "Deploys moved to Tuesday." Session three — a fresh session, empty transcript, memory coming entirely from the replayed journal — I ask what it remembers about deploys. The reply that came back is the whole project in three sentences:

> You deploy on Tuesdays (s2.md:3).
>
> (Note: an earlier memory said Fridays, but that was superseded by the move to Tuesdays.)

That reply is verbatim from the live run (Jul 5), unscripted.

It cites its source. It narrates its own history, unprompted. It doesn't just fail to mention the Friday fact — it *knows* the Friday fact, knows it's dead, and knows what killed it. That's the difference between retrieval ranking and a journal: the forgetting is a typed, receipted event you can replay, audit, and explain, not an embedding that lost a similarity contest.

## Days three and four: the great flattening

Then I looked at the repository and got angry at it. Six crates. Three async transports. tokio, axum, reqwest, and an MCP SDK — sitting on top of a substrate whose whole point is that it ships its own sync operation kit, host composition, receipt verification, and event lanes. I was dogfooding BatPak's journal and hand-rolling everything else it already provided. Mid-hackathon, deadline in three days, I made the call: rebuild from the ground up, all or nothing.

What made that sane instead of suicidal is that texo's identity lives in the math, not the code: a claim ID is a pure function of source, line, and normalized text. So the rebuild had a falsifiable acceptance test — flatten six crates into one, replace every async transport with hand-rolled sync code (HTTP server, SSE, the HTTPS client that talks to DashScope, MCP over stdio), move every surface onto receipt-verified operations, and then check that every claim ID in the golden corpus comes out *identical*. It did. 492 locked dependencies became 222, and the rebuilt system provably remembers everything the old one did.

Sessions got the best upgrade: they moved *into* the journal. Each turn appends to a hidden per-session lane the moment it lands — no in-memory transcript, no separate session store. The journal is the session. The test that made me grin: append two turns, drop the process cold, reopen — both turns are there. Try that with a chat history dict.

Dogfooding cut both ways, too. Mid-rebuild I hit a real gap in my own library — hostbat's 0.9.0 builder couldn't thread status sinks or capability grants — so I filed the issue, and it was fixed upstream the same day. The fix landed in an unpublished 0.10.0, and rather than bet the deadline on a fresh breaking window, the hackathon ships on 0.9.0 *by choice*: a hand-rolled interface fingerprint stands in for the one missing feature, TODO-marked and roadmapped. Finding, filing, and fixing a substrate gap because a downstream project leaned on it hard — that's what the lean was for.

The one-file UI became LiteShip (my Astro-based framework — third dogfood of the week): a chat pane whose replies carry per-claim citation chips, a memory sidebar and journal timeline riding a live SSE stream of the journal's own events, and a receipts panel that cross-checks the binary's interface fingerprint between the stream handshake and the host endpoint. When a supersession lands you watch it happen — the timeline ticks, the old claim slides into the superseded list with an arrow to what killed it.

And the finale, `just drift`: the freshly built binary ingests the repository's *own* markdown — README, specs, the old architecture ADRs, deliberately left standing — and reports which of its claims still hold. The old architecture's claims come back **superseded**, with receipts, by the rebuild ADR that replaced them. texo calling out its own drift is the purity test I didn't know I was building toward: the tool's thesis — prose rots, claims supersede — demonstrated on the tool.

Everything runs on Qwen Cloud through DashScope's OpenAI-compatible mode: `qwen3.7-max` for extraction, the relation judge, and chat; `text-embedding-v4` for the cosine prefilter; the backend on an Alibaba Cloud ECS instance in Singapore, next to the model endpoint.

<!-- TODO: video link -->

Five days, one revert, two live-drive bugs, one ground-up rebuild with a falsifiable identity proof, one upstream gap found and fixed, and an agent that can tell you when it stopped believing something. I'll take that trade.

*— Ayoub*
