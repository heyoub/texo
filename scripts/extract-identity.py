#!/usr/bin/env python3
"""Example external extractor: emit NDJSON claims for texo ingest."""
import json
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as f:
    for line_no, line in enumerate(f, start=1):
        trimmed = line.strip()
        if not trimmed:
            continue
        lower = trimmed.lower()
        if not any(k in lower for k in ("deploy", "decision", "owner", "approval")):
            continue
        subject = "deploy-process"
        predicate = "changed" if any(k in lower for k in ("moved", "changed")) else "unknown"
        obj = "tuesday" if "tuesday" in lower else "friday" if "friday" in lower else lower
        print(
            json.dumps(
                {
                    "line_start": line_no,
                    "text": trimmed,
                    "normalized_text": lower,
                    "subject_hint": subject,
                    "predicate_hint": predicate,
                    "object_hint": obj,
                    "confidence_ppm": 850000,
                }
            )
        )
