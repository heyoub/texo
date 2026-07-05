# texo roadmap / deferred work

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
4. **`texo-extract` (`OutputClaim`, `run_extraction`).** Add the offset fields
   (already available on the span) and emit them in the NDJSON.
5. **Re-bless goldens** — at least `golden_ingest__ingest_demo.snap` and
   `golden_agent_context__agent_context_demo.snap`; review each diff and justify
   it (re-bless, do not weaken).
6. **Tests** — offsets within source bounds; provenance recorded; the
   old-event back-compat decode; a property that `char_end <= source.len()`.

**Risks:** replay back-compat (must test), the two extraction paths agreeing on
field semantics, golden review, and `usize -> u32` conversion/overflow on offsets.

## Relate does not scale to large corpora (O(n²))

**Status:** deferred. Surfaced dogfooding texo on its own docs (145 claims): the
relate pass judges candidate pairs after a cosine prefilter, but the pair set is
still O(n²), so a large corpus is impractical (the Helios demo, ~50 claims, is
fine). Fix: cluster/group claims first (connected components over a similarity
graph — `group_claims` already exists) and relate only *within* a cluster, which
bounds the judge calls to roughly O(n · cluster_size). Until then, semantic
relate is intended for focused workspaces, not whole-repo doc sweeps.

## texo-core replay → WASM (browser replay, portable extension checker)

**Status:** deferred (post-hackathon). The full pipeline cannot target WASM —
`reqwest::blocking` needs sockets, `extractor_cmd` spawns a subprocess, the
BatPak store is real file I/O — and for cloud deployment a native binary is
already maximally portable. But the *replay/projection core* is HTTP-free,
subprocess-free, and float-free by design (integer ppm), so it would compile
to wasm32 cleanly and replay **bit-identically**. Payoffs: VS Code extension
ships a wasm module instead of per-platform binaries; the static trophy page
replays the journal live in-browser. Needs a journal-export format the wasm
module can consume (no BatPak store access from wasm).

## Smaller hardening (review-driven)

- **Source-self-assertion claims.** *Prompt fix landed Jul 4, 2026 (proposer
  exclusion rule + `PROPOSE_PROMPT_VERSION` 3→4); pending live-model validation
  on the next Helios run.* The extractor faithfully recorded meta-claims a
  document makes about *itself* ("this wiki is the source of truth for new
  engineers") as current claims — ironic for a "prose is not state" tool.
- **VS Code extension manners.** *Done Jul 4, 2026:* `execFile` timeout
  (`texo.checkTimeoutMs`, default 30s), per-file trailing-edge debounce,
  status-bar indicator, and a once-per-session notice when
  `.texo/config.toml` is missing.
