import { Theme } from '@czap/core';

/**
 * texo brand theme, light/dark variants. Values mirror colors.tokens.ts —
 * the theme is the coherent multi-variant map, the tokens are the per-name
 * definitions the @token blocks resolve. The export is named `dark` because
 * the `@theme dark` block in Base.astro resolves by export name.
 */
export const dark = Theme.make({
  name: 'brand',
  variants: ['light', 'dark'],
  tokens: {
    primary: { light: '#2f6fb3', dark: '#5c9ce0' },
    secondary: { light: '#0d9488', dark: '#2dd4bf' },
    surface: { light: '#faf9f6', dark: '#0f1115' },
    panel: { light: '#ffffff', dark: '#171a21' },
    text: { light: '#1c1917', dark: '#e7e5e4' },
    muted: { light: '#78716c', dark: '#a8a29e' },
    border: { light: '#e7e5e4', dark: '#2b2f38' },
    current: { light: '#16a34a', dark: '#4ade80' },
    stale: { light: '#d97706', dark: '#fbbf24' },
    conflict: { light: '#dc2626', dark: '#f87171' },
  },
  meta: {
    light: { label: 'Light', mode: 'light' },
    dark: { label: 'Dark', mode: 'dark' },
  },
});
