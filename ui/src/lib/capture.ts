/**
 * MP4 capture — browser-native, encoded by LiteShip's own WebCodecs pipeline
 * (@czap/web → mediabunny mux → video/mp4 Blob). The flex: the submission video
 * is encoded by the same framework that renders the agent's UI.
 *
 * Deterministic: we render each frame at its exact timeline position into an
 * OffscreenCanvas and hand it to the encoder — the output is reproducible, not
 * a wall-clock screen-grab. Chromium + secure context (localhost/https) only;
 * everything degrades to "screen-record the live page" if unavailable.
 */
import { WebCodecsCapture } from '@czap/web';

export interface RecordOptions {
  width: number;
  height: number;
  fps: number;
  durationMs: number;
  /** Paint one frame at timeline position `tMs` onto `ctx`. */
  render: (ctx: OffscreenCanvasRenderingContext2D, tMs: number, frame: number) => void;
  onProgress?: (p: number) => void;
  filename?: string;
  bitrate?: number;
}

export function canRecord(): boolean {
  return typeof (globalThis as any).VideoEncoder !== 'undefined' && typeof OffscreenCanvas !== 'undefined';
}

/** Render the timeline frame-by-frame and download the resulting MP4. */
export async function recordMP4(o: RecordOptions): Promise<void> {
  const w = o.width - (o.width % 2); // H.264 needs even dimensions
  const h = o.height - (o.height % 2);
  const canvas = new OffscreenCanvas(w, h);
  const ctx = canvas.getContext('2d')!;

  const cap = WebCodecsCapture.make({ codec: 'avc1.4D0028', bitrate: o.bitrate ?? 10_000_000 });
  await cap.init({ width: w, height: h, fps: o.fps });

  const total = Math.max(1, Math.round((o.durationMs / 1000) * o.fps));
  for (let i = 0; i < total; i++) {
    const tMs = (i / o.fps) * 1000;
    o.render(ctx, tMs, i);
    await cap.capture({ frame: i, timestamp: Math.round(tMs * 1000), bitmap: canvas as unknown as OffscreenCanvas });
    o.onProgress?.((i + 1) / total);
    if (i % 3 === 0) await new Promise((r) => setTimeout(r, 0)); // keep the tab responsive
  }
  const result = await cap.finalize();
  download(result.blob, o.filename ?? 'texo-ledger.mp4');
}

function download(blob: Blob, name: string): void {
  const url = URL.createObjectURL(blob);
  const a = Object.assign(document.createElement('a'), { href: url, download: name });
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1500);
}
