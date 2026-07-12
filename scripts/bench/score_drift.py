#!/usr/bin/env python3
"""Score a Texo store against a drift oracle (texo.bench.drift.v1, gen v2).

The oracle carries per-layer expectations: --mode heuristic scores the $0
ingest-time path (explicit-signal supersedes pair; implicit/conflict/duplicate
must produce NO phantom relations); --mode semantic scores the full relate
path. Matching is case-insensitive substring, the Helios convention.

Usage: score_drift.py ORACLE CLAIMS_JSON --mode heuristic|semantic
                      [--conflicts CONFLICTS_JSON] [--context CONTEXT_JSON]
Exit 0 always — gates read the report JSON.
"""

import json
import sys


def find(claims, needle):
    n = needle.lower()
    return [c for c in claims if n in c.get("text", "").lower()]


def status_of(claims, needle):
    hits = find(claims, needle)
    if not hits:
        return "absent"
    if any(c.get("status") == "current" for c in hits):
        return "current"
    return hits[0].get("status", "unknown")


def conflict_between(conflicts, claims, t1, t2):
    ids1 = {c["claim_id"] for c in find(claims, t1)}
    ids2 = {c["claim_id"] for c in find(claims, t2)}
    for row in conflicts:
        a, b = row.get("claim_a"), row.get("claim_b")
        if (a in ids1 and b in ids2) or (a in ids2 and b in ids1):
            return True
    return False


def main() -> int:
    argv = sys.argv[1:]
    oracle = json.load(open(argv[0]))
    claims = json.load(open(argv[1]))
    mode = argv[argv.index("--mode") + 1]
    conflicts = []
    if "--conflicts" in argv:
        raw = json.load(open(argv[argv.index("--conflicts") + 1]))
        conflicts = raw.get("open", []) + raw.get("resolved", []) if isinstance(raw, dict) else raw
    context_text = ""
    if "--context" in argv:
        context_text = open(argv[argv.index("--context") + 1]).read().lower()

    per_kind, failures, stale_leaks, phantom_relations = {}, [], 0, 0
    for it in oracle["items"]:
        kind = it["kind"]
        exp = it["expected"][mode]
        k = per_kind.setdefault(kind, {"total": 0, "pass": 0})
        k["total"] += 1
        v1 = status_of(claims, it["v1_text_contains"])
        v2 = status_of(claims, it["v2_text_contains"])
        ok = True

        if kind == "supersede":
            ok = v1 == exp["v1_status"] and v2 == exp["v2_status"]
            if exp.get("conflict") and not conflict_between(
                conflicts, claims, it["v1_text_contains"], it["v2_text_contains"]
            ):
                ok = False
            if mode == "heuristic" and it["signal"] == "implicit" and v1 == "superseded":
                phantom_relations += 1  # implicit pairing shouldn't exist at this layer
        elif kind == "conflict":
            ok = v1 == exp["v1_status"] and v2 == exp["v2_status"]
            has_conf = conflict_between(conflicts, claims,
                                        it["v1_text_contains"], it["v2_text_contains"])
            if exp.get("conflict") and not has_conf:
                ok = False
            if not exp.get("conflict") and has_conf:
                ok = False
                phantom_relations += 1
        elif kind == "duplicate":
            currents = [c for c in find(claims, it["v1_text_contains"])
                        if c.get("status") == "current"]
            if exp.get("both_present_current"):
                ok = len(currents) == 2
            else:
                ok = len(currents) == 1
        elif kind == "unrelated":
            ok = v1 == "current" and v2 == "current"
            if v1 == "superseded" or v2 == "superseded":
                phantom_relations += 1

        if ok:
            k["pass"] += 1
        else:
            failures.append({"id": it["id"], "kind": kind, "signal": it["signal"],
                             "v1_status": v1, "v2_status": v2})
        if context_text and v1 == "superseded":
            if it["v1_text_contains"].lower() in context_text:
                stale_leaks += 1

    report = {
        "schema": "texo.bench.drift.score.v2",
        "mode": mode,
        "corpus_digest": oracle.get("corpus_digest"),
        "generator_version": oracle.get("generator_version"),
        "per_kind": {k: {**v, "recall": round(v["pass"] / v["total"], 4) if v["total"] else None}
                     for k, v in per_kind.items()},
        "explicit_signal_supersede": _signal(oracle, failures, "explicit"),
        "implicit_signal_supersede": _signal(oracle, failures, "implicit"),
        "phantom_relations": phantom_relations,
        "stale_leakage": stale_leaks if context_text else None,
        "failures_total": len(failures),
        "failures_sample": failures[:20],
    }
    json.dump(report, sys.stdout, indent=1)
    print()
    return 0


def _signal(oracle, failures, signal):
    items = [i for i in oracle["items"] if i["kind"] == "supersede" and i["signal"] == signal]
    if not items:
        return None
    failed = {f["id"] for f in failures}
    ok = sum(1 for i in items if i["id"] not in failed)
    return {"total": len(items), "pass": ok, "recall": round(ok / len(items), 4)}


if __name__ == "__main__":
    sys.exit(main())
