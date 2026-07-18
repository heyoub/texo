# ADR-001: Semantic claim pipeline (AST → LLM → embeddings → relation judge)

**Status:** accepted, implemented (optional, opt-in via `[semantics]`).

## Context

texo's claim-chain machinery — append-only BatPak journal, content-addressed
claim IDs, deterministic replay, supersession, conflict detection, receipts — is
sound. But the v0 perception layer (line heuristics + a keyword wordlist + exact
`subject_hint` grouping) **inverts the truth on real docs**: markdown structure
becomes claims, and a later *noise* line supersedes the real decision. Dogfooding
on a deliberately messy corpus ("Helios") scored **1/5**.

Three concerns were conflated in that layer: **structure** (prose vs headings/
code), **meaning** (turning prose into atomic claims), and **relation** (which
claim supersedes which, and what genuinely conflicts).

## Decision

Replace the perception layer with a staged pipeline, **preserving determinism via
a record-once boundary** (the model runs once at ingest; the emitted events are
the recorded output; replay never re-runs a model):

| Stage | Job | Where |
|---|---|---|
| 0 Segment | markdown AST → prose spans (drop headings/code/frontmatter/tables); heading context | `texo-core` (`pulldown-cmark`) |
| 1 Propose | prose span → atomic claims | `texo-extract` bin → OpenRouter, via the existing `extract_via_cmd` seam |
| 2 Ground | deterministic faithfulness gate (token-recall) rejects hallucinations | `texo-core` |
| 3–4 Relate | embed → cosine prefilter → **LLM claim-relation judge**: supersede / conflict / duplicate / unrelated | `texo-cli relate` → `texo-semantics` (OpenRouter) |
| 5 Journal | emit `ClaimRecorded` / `ClaimSuperseded` / `ClaimConflictDetected` | `texo-core` (existing) |

`texo-core` stays HTTP/LLM-free: it defines pure trait seams (`Proposer`,
`Embedder`, `ClaimRelater`, `Nli`) and the hosted OpenRouter backends live in
`texo-semantics`, injected by the binary layer.

## What measurement changed (the load-bearing findings)

The plan assumed embeddings would group by subject and 3-way NLI would relate.
Validating against the **real** models on the Helios sentences disproved both:

1. **Cosine cannot separate subjects that share a token.** "Deploys happen on
   Friday" vs "Releases go out on Friday" embed at **0.875** — essentially equal
   to the **0.881** between the two real *deploy* claims. No single threshold
   separates deploy-Friday from release-Friday.
2. **3-way NLI cannot separate supersession from conflict.** Every value
   replacement (Tuesday vs Friday, Bob vs Alice, BatPak vs Postgres) *and* the
   genuine Monday-vs-Friday disagreement all come back **mutual contradiction**.
   "entailment + recency = supersede" is simply wrong — a new value *negates* the
   old, it does not entail it.

So neither embeddings nor NLI alone can do the relating. We replaced them with a
single richer primitive — a `ClaimRelater` LLM-as-judge that, per candidate pair,
decides shared-subject **and** supersede-vs-conflict at once. Its prompt requires
explicit change wording ("moved to", "now", "no longer") for a supersession; a
bare differing value is a **conflict**, so texo *surfaces disagreements* rather
than silently trusting the newer doc. A coarse cosine prefilter only bounds how
many pairs reach the judge.

## Determinism

Conditional, not cross-machine bit-identity. The model runs once at ingest; its
verdicts become journaled events, and replay/compile are deterministic over them.
A **record-once extraction cache** (content-addressed by `model | prompt_version |
span`) makes re-ingest reproducible, offline, and free (measured 16 s → 4 ms,
byte-identical). Grouping compares by cosine tolerance, never float equality.

## Consequences

- The messy Helios corpus goes **1/5 → 5/5** end-to-end with real models
  (`crates/texo-extract/tests/helios_e2e.rs`, key-gated): Tuesday/Bob/BatPak
  current, Friday/Wednesday/Alice/Postgres retired, Monday⟂Friday a live conflict.
- The heuristic path remains the default; semantics is opt-in per workspace.
- Costs: an OpenRouter dependency (key-gated, cached), and an LLM judge that is
  not bit-deterministic across runs — bounded by the record-once journal.
- Deferred: char-offset + per-model provenance on `ClaimRecorded` (see
  [ROADMAP.md](ROADMAP.md)).
