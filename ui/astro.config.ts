import { fileURLToPath } from 'node:url';
import { defineConfig } from 'astro/config';
import { integration } from '@czap/astro';

const dir = (path: string) => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig({
  // Fully static: every page prerenders to ui/dist and texo's Rust server
  // serves it from disk. There are no Astro API routes — /api/* is texo's
  // own sync HTTP server — so no adapter is needed.
  output: 'static',
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
