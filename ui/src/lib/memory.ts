/**
 * Live memory data layer.
 *
 * Talks to texo's Rust server on the SAME origin (`/api/*`) when served from
 * ECS or a local `texo serve`. On a static host (or offline) every call fails
 * softly and the UI runs on the seeded demo ledger — never blank. This is the
 * `noData` fallback: show the memory, just not a live one.
 */

export interface Claim {
  claim_id: string;
  text: string;
  source_path?: string;
  line?: number;
  state: 'current' | 'stale' | 'conflict';
  superseded_by?: string;
  superseded_by_text?: string;
  receipt?: { event_id?: string; sequence?: number };
}

export interface MemoryState {
  live: boolean;
  frontier: number | null;
  settlement: string;
  current: Claim[];
  stale: Claim[];
  conflicts: Claim[];
}

/** Seeded ledger — real claims from the demo corpus. Used when /api/* is down. */
export const DEMO: MemoryState = {
  live: false,
  frontier: 47,
  settlement: 'seeded',
  current: [
    { claim_id: 'claim_854bcc', text: 'Ben owns release approval now.', source_path: 'meeting_notes.md', line: 5, state: 'current', receipt: { event_id: '42abd965', sequence: 15 } },
    { claim_id: 'claim_2e3b9f', text: 'Deploys moved to Wednesday.', source_path: 'adr_007.md', line: 14, state: 'current', receipt: { event_id: '480bbd6c', sequence: 14 } },
    { claim_id: 'claim_a496fa', text: 'The platform uses BatPak for append-only event storage.', source_path: 'architecture.md', line: 3, state: 'current', receipt: { event_id: '0ad49b50', sequence: 22 } },
  ],
  stale: [
    { claim_id: 'claim_956c3f', text: 'Alice owns release approval.', source_path: 'meeting_notes.md', line: 3, state: 'stale', superseded_by: 'claim_854bcc', superseded_by_text: 'Ben owns release approval now.', receipt: { event_id: '42abd965', sequence: 15 } },
    { claim_id: 'claim_591c69', text: 'Deploys happen on Friday.', source_path: 'onboarding.md', line: 5, state: 'stale', superseded_by: 'claim_2e3b9f', superseded_by_text: 'Deploys moved to Wednesday.', receipt: { event_id: '480bbd6c', sequence: 14 } },
    { claim_id: 'claim_f3aa10', text: 'The platform uses Postgres for storage.', source_path: 'old_arch.md', line: 8, state: 'stale', superseded_by: 'claim_a496fa', superseded_by_text: 'The platform uses BatPak now.', receipt: { event_id: '0ad49b50', sequence: 22 } },
  ],
  conflicts: [
    { claim_id: 'claim_dc00ed', text: 'Release sign-off is verbal. / Release sign-off must be in the ticket.', state: 'conflict' },
  ],
};

const TIMEOUT = 2500;

async function getJSON(path: string): Promise<any | null> {
  try {
    const ctl = new AbortController();
    const t = setTimeout(() => ctl.abort(), TIMEOUT);
    const res = await fetch(path, { signal: ctl.signal, headers: { accept: 'application/json' } });
    clearTimeout(t);
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

function normClaim(c: any, state: Claim['state']): Claim {
  return {
    claim_id: c.claim_id ?? c.id ?? '—',
    text: c.text ?? '',
    source_path: c.source_path ?? c.source?.path,
    line: c.line ?? c.line_start ?? c.source?.line_start,
    state,
    superseded_by: c.superseded_by,
    superseded_by_text: c.superseded_by_text,
    receipt: c.receipt ?? (c.event_id ? { event_id: c.event_id, sequence: c.sequence } : undefined),
  };
}

/** Fetch the live memory; fall back to the seeded ledger on any failure. */
export async function fetchState(): Promise<MemoryState> {
  const [health, mem] = await Promise.all([getJSON('/api/health'), getJSON('/api/memory')]);
  if (!mem) return DEMO;
  return {
    live: true,
    frontier: health?.frontier ?? mem.replayed_through_sequence ?? null,
    settlement: health?.status === 'ok' ? (mem.settlement_complete === false ? 'unsettled' : 'settled') : 'unknown',
    current: (mem.current ?? []).map((c: any) => normClaim(c, 'current')),
    stale: (mem.stale ?? []).map((c: any) => normClaim(c, 'stale')),
    conflicts: (mem.conflicts ?? []).map((c: any) => normClaim(c, 'conflict')),
  };
}

/** Subscribe to the journal SSE stream. Returns a teardown; null if unavailable. */
export function subscribeStream(handlers: {
  onHello?: () => void;
  onSupersede?: (sequence: number) => void;
  onJournal?: (sequence: number, kindBits: number) => void;
  onError?: () => void;
}): (() => void) | null {
  if (typeof EventSource === 'undefined') return null;
  let es: EventSource;
  try {
    es = new EventSource('/api/stream');
  } catch {
    return null;
  }
  es.onmessage = (ev) => {
    try {
      const msg = JSON.parse(ev.data);
      if (msg.type !== 'signal') return;
      const d = msg.data ?? {};
      if (d.kind === 'hello') { handlers.onHello?.(); return; }
      if (d.kind === 'journal') {
        const seq = Number(d.sequence);
        const bits = Number(d.kind_bits);
        handlers.onJournal?.(seq, bits);
        if (bits === 0xe003) handlers.onSupersede?.(seq); // claim superseded
      }
    } catch { /* ignore malformed frame */ }
  };
  es.onerror = () => handlers.onError?.();
  return () => es.close();
}
