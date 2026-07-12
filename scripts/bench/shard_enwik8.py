#!/usr/bin/env python3
"""Deterministic enwik8 sharder: one markdown file per non-redirect wiki page.

Stdlib only. Same input + same SHARDER_VERSION => byte-identical corpus,
proven by the manifest's corpus_digest. Raw wikitext is preserved verbatim
(no conversion) — hostile markup is the point of the benchmark.

Usage: shard_enwik8.py INPUT OUTDIR [--limit N]
"""

import hashlib
import json
import re
import sys
from pathlib import Path

SHARDER_VERSION = "1.0.0"

PAGE_OPEN = b"<page>"
PAGE_CLOSE = b"</page>"
TEXT_CLOSE = b"</text>"


def slug(title: str) -> str:
    s = re.sub(r"[^A-Za-z0-9]+", "_", title).strip("_")
    return s[:80] or "untitled"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def main() -> int:
    argv = sys.argv[1:]
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    input_path, outdir = Path(argv[0]), Path(argv[1])
    limit = None
    if "--limit" in argv:
        limit = int(argv[argv.index("--limit") + 1])

    raw = input_path.read_bytes()
    input_digest = sha256(raw)
    outdir.mkdir(parents=True, exist_ok=True)

    files, skips = [], []
    pos, kept = 0, 0
    while True:
        if limit is not None and kept >= limit:
            break
        start = raw.find(PAGE_OPEN, pos)
        if start == -1:
            break
        end = raw.find(PAGE_CLOSE, start)
        if end == -1:
            skips.append({"page_id": None, "reason": "truncated_tail", "byte_start": start})
            break
        end += len(PAGE_CLOSE)
        page = raw[start:end]
        pos = end

        m_title = re.search(rb"<title>(.*?)</title>", page, re.S)
        m_id = re.search(rb"<id>(\d+)</id>", page)
        title = m_title.group(1).decode("utf-8", "replace") if m_title else ""
        page_id = int(m_id.group(1)) if m_id else None

        m_text = re.search(rb"<text[^>]*>", page)
        if not m_text:
            skips.append({"page_id": page_id, "reason": "no_text"})
            continue
        t_start = m_text.end()
        t_end = page.find(TEXT_CLOSE, t_start)
        if t_end == -1:
            skips.append({"page_id": page_id, "reason": "unclosed_text"})
            continue
        text = page[t_start:t_end]
        if not text.strip():
            skips.append({"page_id": page_id, "reason": "empty_text"})
            continue
        if text.lstrip()[:9].upper().startswith(b"#REDIRECT"):
            skips.append({"page_id": page_id, "reason": "redirect"})
            continue
        if page_id is None:
            skips.append({"page_id": None, "reason": "no_page_id"})
            continue

        name = f"{page_id:09d}-{slug(title)}.md"
        (outdir / name).write_bytes(text)
        files.append({
            "path": name,
            "page_id": page_id,
            "title": title,
            "byte_start": start + t_start,
            "byte_end": start + t_end,
            "sha256": sha256(text),
        })
        kept += 1

    corpus_digest = sha256(
        "\n".join(f"{f['path']}\t{f['sha256']}" for f in sorted(files, key=lambda f: f["path"])).encode()
    )
    manifest = {
        "sharder_version": SHARDER_VERSION,
        "input_sha256": input_digest,
        "limit": limit,
        "page_count": len(files),
        "skip_count": len(skips),
        "corpus_digest": corpus_digest,
        "files": files,
        "skips": skips,
    }
    (outdir / "manifest.json").write_text(json.dumps(manifest, indent=1))
    print(f"sharded {len(files)} pages ({len(skips)} skipped) -> {outdir}")
    print(f"corpus_digest {corpus_digest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
