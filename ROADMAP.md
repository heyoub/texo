# texo roadmap / deferred work

Tracked, deliberately-deferred items. Each entry records *why* it was deferred
and *what it entails*, so picking it up later needs no re-discovery.

## Char-offset + model provenance on `ClaimRecorded`

**Status:** deferred (v1.1). Not needed for the demo; the value is precise
"jump to source" and a per-model audit trail.

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
