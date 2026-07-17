/**
 * The ledger field — texo's domain-native generative backdrop (WebGL2).
 *
 * NOT orbs, NOT a particle network. This is the memory itself: an advancing
 * indigo FRONTIER line, with claim-ticks sedimenting below it (append-only,
 * colored by state — current/stale/conflict), the whole field warming as you
 * scroll (`--mem-heat`). A supersession sends a horizontal strike-ripple.
 *
 * Layered OVER the CSS ruled-paper floor (the canvas is transparent where it
 * draws nothing), so it *adds* the living frontier + marks rather than
 * duplicating the static rules. Strictly an enhancement: it only boots on a
 * GPU-capable tier with motion allowed; otherwise the CSS floor stands alone.
 */
import { readSignalValue } from '@czap/astro/runtime';

const VERT = `#version 300 es
in vec2 a; void main(){ gl_Position = vec4(a, 0.0, 1.0); }`;

const FRAG = `#version 300 es
precision highp float;
out vec4 frag;
uniform vec2  u_res;
uniform float u_time;
uniform float u_heat;     // eased scroll mood 0..1 (frontier height)
uniform float u_strike;   // supersession pulse, decays 1->0
uniform vec3  u_frontier; // indigo
uniform vec3  u_live;     // teal
uniform vec3  u_current;  // green
uniform vec3  u_stale;    // amber
uniform vec3  u_conflict; // red

float hash(vec2 p){ p = fract(p * vec2(123.34, 345.45)); p += dot(p, p + 34.345); return fract(p.x * p.y); }

void main(){
  vec2 px = gl_FragCoord.xy;
  vec2 uv = px / u_res;
  vec3 col = vec3(0.0);
  float a = 0.0;

  // ── the frontier: an indigo line that rises with scroll, glowing ──
  float fy = mix(0.12, 0.94, u_heat) * u_res.y;
  float fd = abs(px.y - fy);
  float front = exp(-fd * fd * 0.0009);
  // subtle horizontal shimmer along the frontier
  float shimmer = 0.6 + 0.4 * sin(px.x * 0.03 + u_time * 1.6);
  col += u_frontier * front * shimmer * 1.15;
  a   += front * 0.5;

  // ── claim-ticks: a sparse grid of marks, sedimented below the frontier ──
  float cell = 46.0;
  vec2 gid = floor(px / cell);
  vec2 gf  = fract(px / cell) - 0.5;
  float h = hash(gid);
  if (h > 0.72) {                                   // ~28% of cells hold a tick
    float below = smoothstep(0.0, 60.0, fy - px.y); // only "recorded" ticks (below frontier) light up
    float twinkle = 0.55 + 0.45 * sin(u_time * 1.2 + h * 40.0);
    float dot = exp(-dot(gf, gf) * 46.0);           // small round mark
    // state by a second hash: mostly dim ink, some current/stale/conflict
    float s = hash(gid + 7.1);
    vec3 c = mix(u_current, u_frontier, 0.35);      // default: quiet current-ish
    if (s > 0.93)      c = u_conflict;
    else if (s > 0.82) c = u_stale;
    else if (s > 0.66) c = u_current;
    else               c = mix(u_frontier, u_live, 0.4) * 0.5;
    float b = dot * below * twinkle;
    col += c * b * 0.9;
    a   += b * 0.32;
  }

  // ── strike-ripple: a bright horizontal sweep on supersession ──
  if (u_strike > 0.001) {
    float sy = mix(0.94, 0.12, 1.0 - u_strike) * u_res.y; // sweeps downward as it decays
    float sd = abs(px.y - sy);
    float strike = exp(-sd * sd * 0.004) * u_strike;
    col += u_stale * strike * 1.4;
    a   += strike * 0.5;
  }

  // ── faint vignette so edges sink into the ink ──
  float vig = smoothstep(1.15, 0.35, length(uv - 0.5));
  a *= mix(0.55, 1.0, vig);

  frag = vec4(col * a, a);   // premultiplied over the CSS floor
}`;

function compile(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader | null {
  const s = gl.createShader(type)!;
  gl.shaderSource(s, src);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) { console.warn('[ledger-field]', gl.getShaderInfoLog(s)); return null; }
  return s;
}

function cssColor(name: string): [number, number, number] {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  const m = v.match(/^#?([0-9a-f]{6})$/i);
  if (m) { const n = parseInt(m[1], 16); return [(n >> 16 & 255) / 255, (n >> 8 & 255) / 255, (n & 255) / 255]; }
  // fall back to a canvas parse for named/other formats
  const c = document.createElement('canvas').getContext('2d')!;
  c.fillStyle = v || '#888'; c.fillRect(0, 0, 1, 1);
  const [r, g, b] = c.getImageData(0, 0, 1, 1).data;
  return [r / 255, g / 255, b / 255];
}

/** Boot the field. Returns a teardown, or null if the tier/device declined it. */
export function initLedgerField(canvas: HTMLCanvasElement): (() => void) | null {
  const root = document.documentElement;
  const tier = root.getAttribute('data-czap-tier') ?? 'reactive';
  const motion = root.getAttribute('data-czap-motion') ?? 'animations';
  const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
  // enhancement only: capable tiers + motion allowed
  if (reduce || motion === 'none' || tier === 'static' || tier === 'styled') return null;

  const gl = canvas.getContext('webgl2', { alpha: true, premultipliedAlpha: true, antialias: false, powerPreference: 'low-power' });
  if (!gl) return null;

  const vs = compile(gl, gl.VERTEX_SHADER, VERT);
  const fs = compile(gl, gl.FRAGMENT_SHADER, FRAG);
  if (!vs || !fs) return null;
  const prog = gl.createProgram()!;
  gl.attachShader(prog, vs); gl.attachShader(prog, fs); gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) { console.warn('[ledger-field]', gl.getProgramInfoLog(prog)); return null; }
  gl.useProgram(prog);

  const buf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(prog, 'a');
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
  gl.enable(gl.BLEND);
  gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);

  const U = {
    res: gl.getUniformLocation(prog, 'u_res'), time: gl.getUniformLocation(prog, 'u_time'),
    heat: gl.getUniformLocation(prog, 'u_heat'), strike: gl.getUniformLocation(prog, 'u_strike'),
    frontier: gl.getUniformLocation(prog, 'u_frontier'), live: gl.getUniformLocation(prog, 'u_live'),
    current: gl.getUniformLocation(prog, 'u_current'), stale: gl.getUniformLocation(prog, 'u_stale'),
    conflict: gl.getUniformLocation(prog, 'u_conflict'),
  };
  gl.uniform3fv(U.frontier, cssColor('--frontier'));
  gl.uniform3fv(U.live, cssColor('--live'));
  gl.uniform3fv(U.current, cssColor('--current'));
  gl.uniform3fv(U.stale, cssColor('--stale'));
  gl.uniform3fv(U.conflict, cssColor('--conflict'));

  const dpr = Math.min(devicePixelRatio || 1, 1.75);
  function resize() {
    const w = Math.floor(innerWidth * dpr), h = Math.floor(innerHeight * dpr);
    canvas.width = w; canvas.height = h; gl!.viewport(0, 0, w, h);
    gl!.uniform2f(U.res, w, h);
  }
  resize();
  addEventListener('resize', resize, { passive: true });

  let heat = 0, strike = 0, t0 = 0, raf = 0, alive = true;
  document.addEventListener('texo:supersede', () => { strike = 1; });

  function frame(ts: number) {
    if (!alive) return;
    if (!t0) t0 = ts;
    const t = (ts - t0) / 1000;
    const target = Math.min(1, Math.max(0, readSignalValue('scroll.progress') ?? 0));
    heat += (target - heat) * 0.06;
    strike *= 0.955;                       // decay the ripple
    gl!.uniform1f(U.time, t);
    gl!.uniform1f(U.heat, heat);
    gl!.uniform1f(U.strike, strike < 0.002 ? 0 : strike);
    gl!.drawArrays(gl!.TRIANGLES, 0, 3);
    raf = requestAnimationFrame(frame);
  }
  raf = requestAnimationFrame(frame);

  // pause when tab hidden (don't re-blur a live canvas for nothing)
  const onVis = () => { if (document.hidden) { alive = false; cancelAnimationFrame(raf); } else if (!alive) { alive = true; t0 = 0; raf = requestAnimationFrame(frame); } };
  document.addEventListener('visibilitychange', onVis);

  root.setAttribute('data-field', 'on');
  return () => { alive = false; cancelAnimationFrame(raf); removeEventListener('resize', resize); document.removeEventListener('visibilitychange', onVis); root.removeAttribute('data-field'); };
}
