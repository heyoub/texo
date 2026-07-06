import { Boundary } from '@czap/core';

/**
 * Viewport-width boundary for the app grid.
 *
 * mobile  [0, 768)    — single column, rail below chat
 * desktop [768, +Inf) — chat + right rail, timeline across the bottom
 *
 * The layout itself is driven by the @quantize cast (compiled container
 * queries, threshold-exact); hysteresis only applies to the satellite's
 * data-czap-state cast, which nothing styles against yet, so none is claimed.
 */
export const layout = Boundary.make({
  input: 'viewport.width',
  at: [
    [0, 'mobile'],
    [768, 'desktop'],
  ],
});
