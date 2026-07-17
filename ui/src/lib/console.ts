/**
 * The teach → settle console — drive a real supersession on-screen.
 *
 * You state facts (POST /api/chat journals them as pending claims this
 * session); then "end session" runs the relate pass (POST /api/session/end),
 * which is where the Qwen judge retires superseded claims and opens conflicts.
 * The rails + HUD above refresh from the live journal, and the backend emits a
 * 0xE003 on the SSE stream — so the ledger field ripples too. This is the whole
 * thesis, performed live: teach a fact, contradict it, watch the old one die
 * with a receipt.
 *
 * Everything fails soft: with no `texo serve`/Qwen key, statements report the
 * gap plainly and the seeded rails stand.
 */

import { fetchState, type MemoryState } from './memory';

interface ConsoleOpts {
  sessionId: string;
  /** hand back fresh live state after a settle so the page re-renders rails+HUD */
  onState?: (s: MemoryState) => void;
}

const short = (s: string, n = 90) => (s.length > n ? s.slice(0, n - 1) + '…' : s);

export function initConsole(root: HTMLElement, opts: ConsoleOpts) {
  const log = root.querySelector<HTMLElement>('#console-log');
  const form = root.querySelector<HTMLFormElement>('#teach');
  const input = root.querySelector<HTMLInputElement>('#teach-in');
  const endBtn = root.querySelector<HTMLButtonElement>('#end-session');
  const out = root.querySelector<HTMLElement>('#settle-out');
  const sidEl = root.querySelector<HTMLElement>('#console-sid');
  const stateEl = root.querySelector<HTMLElement>('#console-state');
  if (!log || !form || !input || !endBtn || !out) return;

  let sessionId = opts.sessionId;
  let staged = 0;
  if (sidEl) sidEl.textContent = sessionId;

  const setState = (s: string) => { if (stateEl) stateEl.textContent = s; };

  function addLine(text: string): HTMLElement {
    log!.querySelector('.console__empty')?.remove();
    const li = document.createElement('li');
    li.className = 'console__line';
    li.innerHTML =
      `<span class="console__prompt" aria-hidden="true">▍</span>` +
      `<span class="console__said"></span>` +
      `<span class="console__meta">journaling…</span>`;
    li.querySelector('.console__said')!.textContent = text;
    log!.appendChild(li);
    log!.scrollTop = log!.scrollHeight;
    return li;
  }
  const setMeta = (li: HTMLElement, text: string, kind = '') => {
    const m = li.querySelector<HTMLElement>('.console__meta')!;
    m.textContent = text;
    m.dataset.kind = kind;
  };

  async function state(msg: string) {
    const li = addLine(msg);
    try {
      const res = await fetch('/api/chat', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ session_id: sessionId, message: msg }),
      });
      const data: any = await res.json().catch(() => ({}));
      // the extractor runs the model synchronously and can hit its deadline —
      // but the turn is still committed to the journal, so it counts at settle.
      if (data.code || data.error) {
        li.classList.add('is-warn');
        setMeta(li, data.committed === 'yes' || data.committed === true ? 'recorded · model slow' : 'model timed out — retry', 'warn');
      } else if (!res.ok) {
        li.classList.add('is-err');
        setMeta(li, 'needs a running texo serve + Qwen key', 'err');
        return;
      } else {
        li.classList.add('is-journaled');
        setMeta(li, data.reply ? `journaled · ${short(String(data.reply), 60)}` : 'journaled', 'ok');
      }
      staged++;
      endBtn!.disabled = false;
      setState(`open · ${staged} staged`);
    } catch {
      li.classList.add('is-err');
      setMeta(li, 'needs a running texo serve + Qwen key', 'err');
    }
  }

  form.addEventListener('submit', (e) => {
    e.preventDefault();
    const msg = input!.value.trim();
    if (!msg) return;
    input!.value = '';
    state(msg);
  });

  // one-click contradiction pair for the demo / video
  root.querySelectorAll<HTMLButtonElement>('.console__eg').forEach((chip) => {
    chip.addEventListener('click', () => { state(chip.dataset.say || chip.textContent!.trim()); });
  });

  endBtn.addEventListener('click', async () => {
    endBtn.disabled = true;
    out.hidden = false;
    out.className = 'console__receipt is-pending';
    out.textContent = 'settling — running the relation judge…';
    try {
      const res = await fetch('/api/session/end', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ session_id: sessionId }),
      });
      if (!res.ok) throw new Error(String(res.status));
      const data = await res.json().catch(() => ({}));
      const r = data.relate ?? {};
      const num = (v: any) => (Array.isArray(v) ? v.length : Number(v ?? 0)) || 0;
      const recorded = num(data.claims_recorded ?? data.claims);
      const sup = num(r.supersessions);
      const con = num(r.conflicts);
      out.className = 'console__receipt is-sealed';
      out.innerHTML =
        `<span class="console__seal-tag mono">session sealed</span>` +
        `<span class="console__seal-nums">` +
          `<b>${recorded}</b> claims · ` +
          `<b class="n-stale">${sup}</b> superseded · ` +
          `<b class="n-con">${con}</b> conflict${con === 1 ? '' : 's'}` +
        `</span>`;
      setState('sealed');
      log!.querySelectorAll('.console__line').forEach((l) => l.classList.add('is-sealed'));
      // refresh the live ledger (rails + HUD); the SSE 0xE003 ripples the field
      const s = await fetchState();
      opts.onState?.(s);
      // roll a fresh session so you can run another round
      staged = 0;
      sessionId = `ui-${(crypto as any).randomUUID?.() ?? Date.now()}`;
      if (sidEl) sidEl.textContent = sessionId;
    } catch {
      out.className = 'console__receipt is-err';
      out.textContent = 'settle needs a running texo serve with a Qwen key — the rails above are still live-replayed.';
      endBtn.disabled = staged === 0;
    }
  });
}
