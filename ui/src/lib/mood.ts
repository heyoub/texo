/**
 * The ledger mood engine.
 *
 * Reads the canonical `scroll.progress` signal via the @czap runtime (rAF-
 * throttled, resize-safe — not a hand-rolled scroll listener) and casts it to
 * the page as (a) eased `--mem-heat` / `--mem-frontier` custom properties and
 * (b) a discrete `data-mood` state on <html>. The four moods are the journal's
 * arc: settling → accumulating → superseding → sealed.
 *
 * Easing is a critically-damped lerp (the charge-rail trick) so the cast glides
 * to the target instead of snapping — the same feel @czap/quantizer's
 * AnimatedQuantizer gives, without pulling in the extra package. Phase 2 swaps
 * this for the real quantizer to also drive the WebGL cast.
 */
import { attachSignalObserver, readSignalValue } from '@czap/astro/runtime';

type Mood = 'settling' | 'accumulating' | 'superseding' | 'sealed';

const MOODS: readonly [number, Mood][] = [
  [0, 'settling'],
  [0.28, 'accumulating'],
  [0.62, 'superseding'],
  [0.9, 'sealed'],
];

function moodFor(p: number): Mood {
  let m: Mood = 'settling';
  for (const [t, name] of MOODS) if (p >= t) m = name;
  return m;
}

export function initMood(): void {
  const root = document.documentElement;
  const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;

  let target = clampProgress(readSignalValue('scroll.progress'));
  let heat = target;
  let mood: Mood | '' = '';

  const setMood = (next: Mood) => {
    if (next === mood) return;
    mood = next;
    root.setAttribute('data-mood', next);
    root.dispatchEvent(new CustomEvent('texo:mood', { detail: { mood: next }, bubbles: true }));
  };

  if (reduce) {
    // No per-frame easing under reduced motion: land on the value, keep it truthful.
    const paint = () => {
      const p = clampProgress(readSignalValue('scroll.progress'));
      root.style.setProperty('--mem-heat', p.toFixed(4));
      root.style.setProperty('--mem-frontier', p.toFixed(4));
      setMood(moodFor(p));
    };
    attachSignalObserver('scroll.progress', paint);
    paint();
    return;
  }

  attachSignalObserver('scroll.progress', () => {
    target = clampProgress(readSignalValue('scroll.progress'));
  });

  const tick = () => {
    // critically-damped approach; halts itself when settled to save frames
    const d = target - heat;
    if (Math.abs(d) > 0.0004) {
      heat += d * 0.12;
      root.style.setProperty('--mem-heat', heat.toFixed(4));
      root.style.setProperty('--mem-frontier', heat.toFixed(4));
      setMood(moodFor(heat));
      requestAnimationFrame(tick);
    } else {
      heat = target;
      root.style.setProperty('--mem-heat', heat.toFixed(4));
      root.style.setProperty('--mem-frontier', heat.toFixed(4));
      setMood(moodFor(heat));
      running = false;
    }
  };

  let running = false;
  const kick = () => { if (!running) { running = true; requestAnimationFrame(tick); } };
  attachSignalObserver('scroll.progress', kick);
  kick();
}

function clampProgress(v: number | undefined): number {
  if (v === undefined || Number.isNaN(v)) return 0;
  return Math.min(1, Math.max(0, v));
}
