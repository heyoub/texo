#!/usr/bin/env python3
"""Deterministic drift overlay for a sharded corpus: v1/v2 file sets + oracle.

Injects controlled declarative sentences (explicit third-person subjects —
extraction is phrasing-sensitive, so the oracle only asserts what the
injected sentences carry) into per-article files; the article text supplies
realistic surrounding noise. The wikitext itself is never mutated.

Kinds (per 10 items): 4 supersede (2 explicit-signal, 2 implicit),
2 conflict (v2a/v2b), 1 duplicate, 3 unrelated.

Usage: gen_drift.py CORPUS_DIR OUT_DIR [--items N] [--seed S]
Oracle schema: texo.bench.drift.v1
"""

import hashlib
import json
import random
import sys
from pathlib import Path

GENERATOR_VERSION = "2.0.0"
# v2: templates aligned with Texo's actual heuristics (verified in source):
# - every sentence carries an extraction trigger word ("is"/"uses") so it
#   reliably becomes a claim (extract/heuristics.rs has_claim_signal);
# - subjects avoid the four bucket words (deploy/release/owner|owns/approval)
#   that detect_subject collapses — bucket collisions cause cross-item
#   pairing chaos (proven by the v1 smoke run: 18/20 scrambled);
# - per-item uniqueness lives INSIDE the first six normalized words (that is
#   what slugify_words keys the subject_hint on), and supersede v1/v2 share
#   those six words exactly, with the changing object after word six;
# - explicit supersede v2 uses "is now" — "now" is a replacement_signal word,
#   "is" keeps first-six-word equality with v1.
# Heuristic-layer expectations encoded in the oracle: implicit supersedes,
# conflicts, and duplicate-marking are SEMANTIC-path work; at the heuristic
# layer they must simply not produce phantom relations.

NAMES = ["Aldera", "Borealis", "Corvid", "Datura", "Ekliptik", "Ferrite",
         "Gossamer", "Halcyon", "Ilex", "Juniper", "Kestrel", "Lumen",
         "Meridian", "Nocturne", "Obsidian", "Palisade"]
DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday"]
OWNERS = ["Alice", "Bob", "Carol", "Dmitri", "Esther", "Farid"]
STORES = ["Postgres", "BatPak", "Redis", "SQLite"]


def main() -> int:
    argv = sys.argv[1:]
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    corpus, out = Path(argv[0]), Path(argv[1])
    items_n = int(argv[argv.index("--items") + 1]) if "--items" in argv else 1000
    seed = int(argv[argv.index("--seed") + 1]) if "--seed" in argv else 42
    rng = random.Random(seed)

    manifest = json.loads((corpus / "manifest.json").read_text())
    files = sorted(manifest["files"], key=lambda f: f["path"])[:items_n]
    if len(files) < items_n:
        print(f"corpus has only {len(files)} files; generating that many", file=sys.stderr)
        items_n = len(files)

    v1_dir, v2_dir = out / "v1", out / "v2"
    v1_dir.mkdir(parents=True, exist_ok=True)
    v2_dir.mkdir(parents=True, exist_ok=True)

    kinds = (["supersede_explicit", "supersede_explicit", "supersede_implicit",
              "supersede_implicit", "conflict", "conflict", "duplicate",
              "unrelated", "unrelated", "unrelated"])
    oracle_items = []
    for i, f in enumerate(files):
        kind = kinds[i % len(kinds)]
        noise = (corpus / f["path"]).read_bytes().decode("utf-8", "replace")
        noise = "\n".join(noise.splitlines()[:40])
        name = f"{NAMES[rng.randrange(len(NAMES))]}-{i:04d}"
        item_id = f"drift-{i:06d}"

        if kind.startswith("supersede"):
            d1, d2 = rng.sample(DAYS, 2)
            # first six normalized words identical: "<name> sync window target day is"
            v1_s = f"{name} sync window target day is {d1}."
            if kind == "supersede_explicit":
                v2_s = f"{name} sync window target day is now {d2}."
                expected = {"heuristic": {"v1_status": "superseded", "v2_status": "current"},
                            "semantic": {"v1_status": "superseded", "v2_status": "current"}}
            else:
                v2_s = f"{name} sync window target day is {d2}."
                expected = {"heuristic": {"v1_status": "current", "v2_status": "current"},
                            "semantic": {"v1_status": "superseded", "v2_status": "current"}}
            signal = "explicit" if kind == "supersede_explicit" else "implicit"
            kind_out = "supersede"
        elif kind == "conflict":
            s1, s2 = rng.sample(STORES, 2)
            v1_s = f"{name} archive layer uses the {s1} storage engine."
            v2_s = f"{name} archive layer uses the {s2} storage engine."
            expected = {"heuristic": {"v1_status": "current", "v2_status": "current",
                                      "conflict": False},
                        "semantic": {"v1_status": "current", "v2_status": "current",
                                     "conflict": True}}
            signal, kind_out = "none", "conflict"
        elif kind == "duplicate":
            o = OWNERS[rng.randrange(len(OWNERS))]
            v1_s = f"{name} rotation duty holder is {o}."
            v2_s = v1_s
            expected = {"heuristic": {"both_present_current": True},
                        "semantic": {"single_current": True}}
            signal, kind_out = "none", "duplicate"
        else:
            d = DAYS[rng.randrange(len(DAYS))]
            other = f"{NAMES[rng.randrange(len(NAMES))]}alt{i:04d}"
            v1_s = f"{name} maintenance audit cadence is {d}."
            v2_s = f"{other} backup retention period is quarterly."
            expected = {"heuristic": {"v1_status": "current", "v2_status": "current",
                                      "no_relation": True},
                        "semantic": {"v1_status": "current", "v2_status": "current",
                                     "no_relation": True}}
            signal, kind_out = "none", "unrelated"

        v1_name, v2_name = f"{item_id}-v1.md", f"{item_id}-v2.md"
        (v1_dir / v1_name).write_text(f"# {name} notes\n\n{v1_s}\n\n{noise}\n")
        (v2_dir / v2_name).write_text(f"# {name} update\n\n{v2_s}\n")
        oracle_items.append({
            "id": item_id, "kind": kind_out, "signal": signal,
            "page_id": f["page_id"],
            "v1_file": f"v1/{v1_name}", "v2_file": f"v2/{v2_name}",
            "v1_text_contains": v1_s, "v2_text_contains": v2_s,
            "expected": expected,
        })

    totals = {}
    for it in oracle_items:
        totals[it["kind"]] = totals.get(it["kind"], 0) + 1
    oracle = {
        "schema": "texo.bench.drift.v1",
        "generator_version": GENERATOR_VERSION,
        "seed": seed,
        "corpus_digest": manifest["corpus_digest"],
        "totals": totals,
        "items": oracle_items,
    }
    (out / "oracle.json").write_text(json.dumps(oracle, indent=1))
    digest = hashlib.sha256((out / "oracle.json").read_bytes()).hexdigest()
    print(f"generated {len(oracle_items)} drift items -> {out} (totals {totals})")
    print(f"oracle_digest {digest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
