import { fileURLToPath } from 'node:url';
import { defineConfig, fontProviders } from 'astro/config';
import { integration } from '@czap/astro';

const dir = (path: string) => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig({
  // Fully static: every page prerenders to ui/dist and texo's Rust server
  // serves it from disk. There are no Astro API routes — /api/* is texo's
  // own sync HTTP server — so no adapter is needed.
  output: 'static',
  // Self-hosted at build time (Astro Fonts API): zero-CLS, metric-matched
  // fallbacks, no runtime Google round-trip. Three voices —
  // serif = the human claim, mono = the machine receipt/hash, sans = prose.
  fonts: [
    { provider: fontProviders.google(), name: 'Instrument Serif', cssVariable: '--font-instrument-serif', weights: [400], styles: ['normal', 'italic'] },
    { provider: fontProviders.google(), name: 'Space Mono', cssVariable: '--font-space-mono', weights: [400, 700] },
    { provider: fontProviders.google(), name: 'DM Sans', cssVariable: '--font-dm-sans', weights: [300, 400, 500, 600] },
  ],
  // Flat files (drift.html, not drift/index.html): texo's static route does
  // exact path joins with a single "/" → index.html special case.
  build: { format: 'file' },
  integrations: [
    integration({
      vite: {
        dirs: {
          boundary: dir('./src/boundaries'),
          token: dir('./src/tokens'),
          theme: dir('./src/themes'),
        },
      },
    }),
  ],
});
