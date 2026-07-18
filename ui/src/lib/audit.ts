/**
 * The self-audit data layer — texo remembers texo.
 *
 * Reads `/self-audit.json`: the real output of `texo claims --json` run over
 * texo's OWN repo docs (see `just self-audit`). Unlike `/api/memory`, this
 * snapshot carries source path + line + a receipt for EVERY claim, current or
 * superseded — which is what lets the audit cross-highlight a superseded claim
 * back to the exact line it was retired from. The corpus docs are snapshotted
 * alongside at `/corpus/<file>` so the source pane shows exactly what was
 * ingested. Regenerated each build/deploy; ships static, no backend needed.
 */

export interface AuditClaim {
  claim_id: string;
  text: string;
  status: 'current' | 'superseded' | string;
  subject_hint?: string;
  file: string;                 // basename, e.g. "ARCHITECTURE.md"
  line: number | null;          // 1-based
  receipt?: { event_id?: string; sequence?: number } | null;
  superseded_by?: string | null;
}

export interface AuditDoc {
  file: string;
  path: string;                 // "/corpus/ARCHITECTURE.md"
  claims: AuditClaim[];         // sorted by line
  current: number;
  superseded: number;
}

export interface AuditData {
  docs: AuditDoc[];
  byId: Map<string, AuditClaim>;
  totals: { claims: number; current: number; superseded: number; docs: number };
}

export async function loadAudit(): Promise<AuditData | null> {
  let raw: any;
  try {
    const res = await fetch('/self-audit.json', { headers: { accept: 'application/json' } });
    if (!res.ok) return null;
    raw = await res.json();
  } catch {
    return null;
  }
  const arr: any[] = Array.isArray(raw) ? raw : (raw?.claims ?? []);
  if (!arr.length) return null;

  const byId = new Map<string, AuditClaim>();
  const norm: AuditClaim[] = arr.map((c) => {
    const source = c.source ?? {};
    const file = String(source.path ?? c.source_path ?? '').split('/').pop() ?? '';
    const claim: AuditClaim = {
      claim_id: c.claim_id ?? c.id ?? '—',
      text: c.text ?? '',
      status: c.status ?? 'current',
      subject_hint: c.subject_hint,
      file,
      line: source.line_start ?? source.line ?? c.line ?? null,
      receipt: c.receipt ?? (c.event_id ? { event_id: c.event_id, sequence: c.sequence } : null),
      superseded_by: c.superseded_by ?? null,
    };
    byId.set(claim.claim_id, claim);
    return claim;
  });

  const docsMap = new Map<string, AuditClaim[]>();
  for (const c of norm) {
    if (!c.file) continue;
    const list = docsMap.get(c.file) ?? [];
    list.push(c);
    docsMap.set(c.file, list);
  }

  const docs: AuditDoc[] = [...docsMap.entries()]
    .map(([file, claims]) => ({
      file,
      path: `/corpus/${file}`,
      claims: claims.sort((a, b) => (a.line ?? 0) - (b.line ?? 0)),
      current: claims.filter((c) => c.status === 'current').length,
      superseded: claims.filter((c) => c.status !== 'current').length,
    }))
    // docs with the richest supersession story first, then by volume
    .sort((a, b) => b.superseded - a.superseded || b.claims.length - a.claims.length);

  const totals = {
    claims: norm.length,
    current: norm.filter((c) => c.status === 'current').length,
    superseded: norm.filter((c) => c.status !== 'current').length,
    docs: docs.length,
  };

  return { docs, byId, totals };
}
