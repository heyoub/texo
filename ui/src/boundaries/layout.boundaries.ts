import { Boundary } from '@czap/core';

/**
 * Viewport-width boundary for the app grid.
 *
 * mobile  [0, 768)    — single column, rail below chat
 * desktop [768, +Inf) — chat + right rail, timeline across the bottom
 *
 * Hysteresis of 40px prevents jitter at the threshold.
 */
export const layout = Boundary.make({
  input: 'viewport.width',
  at: [
    [0, 'mobile'],
    [768, 'desktop'],
  ],
  hysteresis: 40,
});
