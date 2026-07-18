# texo roadmap / deferred work

Supersedes: see [ADR-003](ADR-003-single-crate-rebuild.md).

Tracked, deliberately-deferred items. Each entry records *why* it was deferred
and *what it entails*, so picking it up later needs no re-discovery.

*Active (not deferred) work for the Qwen Cloud hackathon lives in
[HACKATHON.md](HACKATHON.md).*

## Char-offset + model provenance on `ClaimRecorded`

**Status:** done (v1.1). `ClaimRecorded` now carries `char_start`/`char_end`
(the source **span's** byte range) plus `extractor_model`/`prompt_version`, all
`#[serde(default)]` so pre-v1.1 journals replay unchanged; both extraction
paths populate them and the NDJSON seam threads them through. The entry below
is kept for the design rationale.

**Why deferred:** it is a *versioned event-schema change* with a back-compat
obligation, and it touches both extraction paths plus several goldens — real
work that the shippable demo does not require. The determinism story is already
covered without it (the record-once cache pins model + prompt version via the
`Proposer::fingerprint`, and `extractor_kind` already records which extractor
produced a claim).

**Design note:** an LLM claim is a *paraphrase*, so the claim text is not
guaranteed to be a verbatim substring of its source — claim-level char offsets
are ill-defined. The defensible semantics is to attribute the **source span's**
byte range (Stage 0 `CandidateSpan` already computes `char_start`/`char_end`) to
each claim it produced: "this claim came from bytes X–Y of this doc."

**What it entails:**

1. **Event payload (`events/payloads.rs`).** Add `char_start: u32`,
   `char_end: u32`, and provenance (`extractor_model`, `prompt_version`) to
   `ClaimRecorded`. The new fields MUST be `#[serde(default)]` — events are
   already journaled without them and there is no event-version gate or
   `deny_unknown_fields`, so default-on-decode is what keeps **old journals
   replayable**. Add a regression test that an old-format event still decodes.
   `claim_id` is `hash(source_id, line, normalized)`, so IDs do **not** churn.
2. **Heuristic path (`source/markdown.rs`, `extract/`).** `MarkdownLine` is only
   `{number, text}` — no byte offset. To populate offsets on the default
   (non-LLM) path, extend `parse_lines` to track each line's byte position and
   thread it through `extract_claims`/`build_claim`. (Otherwise the two paths
   disagree.)
3. **cmd seam (`extract/cmd.rs`).** Add optional `char_start`/`char_end` to
   `CmdClaimLine`; thread into `ClaimRecorded` (default when absent).
4. **`texo extract` (`OutputClaim`, `run_extraction`).** Add the offset fields
   (already available on the span) and emit them in the NDJSON.
5. **Re-bless goldens** — at least `golden_ingest__ingest_demo.snap` and
   `golden_agent_context__agent_context_demo.snap`; review each diff and justify
   it (re-bless, do not weaken).
6. **Tests** — offsets within source bounds; provenance recorded; the
   old-event back-compat decode; a property that `char_end <= source.len()`.

**Risks:** replay back-compat (must test), the two extraction paths agreeing on
field semantics, golden review, and `usize -> u32` conversion/overflow on offsets.

## texo replay -> WASM (browser replay, portable extension checker)

**Status:** deferred (post-hackathon). The full pipeline cannot target WASM —
the OpenAI-compatible client needs sockets, `extractor_cmd` spawns a subprocess, the
BatPak store is real file I/O — and for cloud deployment a native binary is
already maximally portable. But the *replay/projection core* is HTTP-free,
subprocess-free, and float-free by design (integer ppm), so it would compile
to wasm32 cleanly and replay **bit-identically**. Payoffs: VS Code extension
ships a wasm module instead of per-platform binaries; the static trophy page
replays the journal live in-browser. Needs a journal-export format the wasm
module can consume (no BatPak store access from wasm).

## Smaller hardening (review-driven)

- **Source-self-assertion claims.** *Attempted and reverted Jul 4, 2026:* a
  proposer exclusion bullet (v4) shifted extraction wording broadly enough to
  drop Helios to 4/5 live (the Postgres→BatPak supersession degraded to a
  conflict — plausibly a wording→embedding→cluster-split interaction), so it
  was reverted to v3 same day. Lesson: prompt changes are pipeline changes —
  they must be iterated against the live oracle before landing, and ideally
  with a self-assertion case added to the oracle corpus first. The underlying
  issue stands: the extractor faithfully records meta-claims a document makes
  about *itself* ("this wiki is the source of truth for new engineers") as
  current claims — ironic for a "prose is not state" tool.
- **VS Code extension manners.** *Done Jul 4, 2026:* `execFile` timeout
  (`texo.checkTimeoutMs`, default 30s), per-file trailing-edge debounce,
  status-bar indicator, and a once-per-session notice when
  `.texo/config.toml` is missing.

## Post-Rebuild Closeout Items

- **batpak 0.10 bump + hostbat manifest re-mount.** Replace the interim
  `texo-canonical-v1` fingerprint with hostbat `HostModule` manifests once
  texo moves past the 0.9.0 HostBuilder gap
  (freebatteryfactory/batpak#166, fixed in #169/0.10.0).
- **MemFs-backed test stores + SimFs crash tests (0.10).** Move the heavy
  real-store tests onto family-provided memory stores where possible, and use
  SimFs for crash-window proofs.
- **SSE replay/snapshot endpoints.** Add LiteShip resumption support beyond
  the current `/api/memory` re-sync plus live `/api/stream` signal feed.
- **texo replay -> WASM.** The existing browser replay item becomes
  stronger once MemFs-backed stores can provide a small journal-export reader.
