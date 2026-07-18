import { Token } from '@czap/core';

/**
 * texo palette — paper-and-ink journal aesthetic with claim-status colors.
 * Each token compiles to a `--czap-<name>` custom property via the @token
 * blocks in Base.astro.
 */

export const primary = Token.make({
  name: 'primary',
  category: 'color',
  // the FRONTIER — structural "fresh-ink" line. Deliberately azure, NOT the
  // periwinkle #818cf8 (reads as generic-AI indigo) and kept distinct from the
  // warm paper text ramp so the current-line never blends into body copy.
  axes: ['theme'],
  values: { light: '#2f6fb3', dark: '#5c9ce0' },
  fallback: '#2f6fb3',
});

export const secondary = Token.make({
  name: 'secondary',
  category: 'color',
  axes: ['theme'],
  values: { light: '#0d9488', dark: '#2dd4bf' },
  fallback: '#0d9488',
});

export const surface = Token.make({
  name: 'surface',
  category: 'color',
  axes: ['theme'],
  values: { light: '#faf9f6', dark: '#0f1115' },
  fallback: '#faf9f6',
});

export const panel = Token.make({
  name: 'panel',
  category: 'color',
  axes: ['theme'],
  values: { light: '#ffffff', dark: '#171a21' },
  fallback: '#ffffff',
});

export const text = Token.make({
  name: 'text',
  category: 'color',
  axes: ['theme'],
  values: { light: '#1c1917', dark: '#e7e5e4' },
  fallback: '#1c1917',
});

export const muted = Token.make({
  name: 'muted',
  category: 'color',
  axes: ['theme'],
  values: { light: '#78716c', dark: '#a8a29e' },
  fallback: '#78716c',
});

export const border = Token.make({
  name: 'border',
  category: 'color',
  axes: ['theme'],
  values: { light: '#e7e5e4', dark: '#2b2f38' },
  fallback: '#e7e5e4',
});

/** Claim status: current (live truth). */
export const current = Token.make({
  name: 'current',
  category: 'color',
  axes: ['theme'],
  values: { light: '#16a34a', dark: '#4ade80' },
  fallback: '#16a34a',
});

/** Claim status: superseded / stale. */
export const stale = Token.make({
  name: 'stale',
  category: 'color',
  axes: ['theme'],
  values: { light: '#d97706', dark: '#fbbf24' },
  fallback: '#d97706',
});

/** Claim status: conflicting. */
export const conflict = Token.make({
  name: 'conflict',
  category: 'color',
  axes: ['theme'],
  values: { light: '#dc2626', dark: '#f87171' },
  fallback: '#dc2626',
});
