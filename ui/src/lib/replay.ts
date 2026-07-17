/**
 * The horizontal replay — a real journal chain, scrubbed by scroll.
 *
 * The tape is the append-only ledger unspooling left→right; a fixed indigo
 * PLAYHEAD sits mid-stage and beats cross under it as you scroll. Because every
 * bit of state (strike width, receipt tear, new-claim rise, sediment sink, HUD
 * counters) is a pure function of scroll progress, scrolling BACK reverses it —
 * you can scrub a supersession in either direction, like a timeline scrubber.
 *
 * The data is real: these are the actual supersession chains texo produces on
 * the demo corpus (deploys Friday→Wednesday, Alice→Ben, Postgres→BatPak), with
 * their real receipt addresses and sequence numbers.
 *
 * Progressive enhancement: with no JS, or under reduced-motion / a static GPU
 * tier, the section stays in `flow` mode — a plain horizontally-scrollable strip
 * the user swipes themselves (no scroll-jacking). JS upgrades motion-OK clients
 * to `pin` mode (sticky stage + scroll-driven tape).
 */

export type Beat =
  | { seq: number; type: 'append'; text: string; src: string }
  | { seq: number; type: 'supersede'; text: string; src: string; retires: string; receipt: string }
  | { seq: number; type: 'conflict'; a: string; b: string };

/** Real chains from the demo corpus — ordered as a session replay. */
export const REEL: Beat[] = [
  { seq: 3,  type: 'append',    text: 'The platform uses Postgres for storage.', src: 'old_arch.md:8' },
  { seq: 5,  type: 'append',    text: 'Deploys happen on Friday.',               src: 'onboarding.md:5' },
  { seq: 9,  type: 'append',    text: 'Alice owns release approval.',            src: 'meeting_notes.md:3' },
  { seq: 14, type: 'supersede', text: 'Deploys moved to Wednesday.',            src: 'adr_007.md:14',  retires: 'Deploys happen on Friday.',        receipt: '480bbd6c' },
  { seq: 15, type: 'supersede', text: 'Ben owns release approval now.',         src: 'meeting_notes.md:5', retires: 'Alice owns release approval.',  receipt: '42abd965' },
  { seq: 18, type: 'conflict',  a: 'Release sign-off is verbal.',               b: 'Release sign-off must be in the ticket.' },
  { seq: 22, type: 'supersede', text: 'The platform uses BatPak for append-only event storage.', src: 'architecture.md:3', retires: 'The platform uses Postgres for storage.', receipt: '0ad49b50' },
  { seq: 24, type: 'append',    text: 'Onboarding uses the current checklist.', src: 'current_architecture.md:9' },
];

export interface Tally { frontier: number; current: number; stale: number; conflict: number }

interface ReplayOpts {
  /** active beat committed at the playhead (index, running tallies). */
  onActive?: (index: number, tally: Tally) => void;
  /** scrolled out / before the reel — hand the HUD back to live state. */
  onIdle?: () => void;
}

const clamp01 = (n: number) => (n < 0 ? 0 : n > 1 ? 1 : n);

export function initReplay(section: HTMLElement, opts: ReplayOpts = {}) {
  const scroller = section.querySelector<HTMLElement>('.replay__scroller');
  const stage = section.querySelector<HTMLElement>('.replay__stage');
  const tape = section.querySelector<HTMLElement>('.replay__tape');
  if (!scroller || !stage || !tape) return;
  const beats = Array.from(tape.querySelectorAll<HTMLElement>('.rbeat'));
  if (!beats.length) return;

  const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const tier = document.documentElement.getAttribute('data-czap-tier');
  const staticTier = tier === 'static' || tier === 'styled';

  // Fallback: user-driven horizontal scroll, everything resolved. No hijack.
  if (reduce || staticTier) {
    section.setAttribute('data-mode', 'flow');
    return;
  }

  section.setAttribute('data-mode', 'pin');

  const HEAD = 0.5;             // playhead at mid-stage
  let headX = 0, maxScroll = 1;

  function measure() {
    headX = stage!.clientWidth * HEAD;
    const last = beats[beats.length - 1];
    const lastCenter = last.offsetLeft + last.offsetWidth / 2;
    maxScroll = Math.max(1, lastCenter - headX);
    scroller!.style.height = stage!.clientHeight + maxScroll + 'px';
  }

  function tally(active: number): Tally {
    let current = 0, stale = 0, conflict = 0, frontier = 0;
    for (let i = 0; i <= active && i < beats.length; i++) {
      const t = beats[i].dataset.type;
      frontier = Number(beats[i].dataset.seq) || frontier;
      if (t === 'append') current++;
      else if (t === 'supersede') { current++; stale++; }
      else if (t === 'conflict') conflict++;
    }
    return { frontier, current, stale, conflict };
  }

  let lastActive = -99;
  let wasVisible = false;
  let queued = false;

  function apply() {
    queued = false;
    const r = scroller!.getBoundingClientRect();
    const visible = r.bottom > 0 && r.top < innerHeight;
    if (!visible) {
      if (wasVisible) { wasVisible = false; lastActive = -99; opts.onIdle?.(); }
      return;
    }
    if (!wasVisible) { wasVisible = true; lastActive = -99; }

    const progress = clamp01(-r.top / maxScroll);
    const tx = -progress * maxScroll;
    tape!.style.transform = `translate3d(${tx}px,0,0)`;

    let active = -1;
    const half = stage!.clientWidth * 0.5;
    for (let i = 0; i < beats.length; i++) {
      const b = beats[i];
      const screenLeft = b.offsetLeft + tx;
      const w = b.offsetWidth;
      const rev = clamp01((headX - screenLeft) / w);
      const emph = clamp01(1 - Math.abs(screenLeft + w / 2 - headX) / half);
      b.style.setProperty('--reveal', rev.toFixed(4));
      b.style.setProperty('--emph', emph.toFixed(4));
      b.classList.toggle('is-head', rev > 0.35 && rev < 0.98);
      if (screenLeft + w / 2 <= headX) active = i;
    }

    if (active !== lastActive) {
      if (active > lastActive && lastActive >= -1) {
        for (let i = lastActive + 1; i <= active; i++) {
          if (beats[i]?.dataset.type === 'supersede') {
            document.dispatchEvent(new CustomEvent('texo:supersede'));
          }
        }
      }
      lastActive = active;
      if (active < 0) opts.onIdle?.();
      else opts.onActive?.(active, tally(active));
    }
  }

  function onScroll() {
    if (!queued) { queued = true; requestAnimationFrame(apply); }
  }

  measure();
  addEventListener('scroll', onScroll, { passive: true });
  addEventListener('resize', () => { measure(); onScroll(); }, { passive: true });
  (document as any).fonts?.ready?.then(() => { measure(); onScroll(); });
  onScroll();
}
