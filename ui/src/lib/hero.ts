/**
 * The hero scene — a deterministic ~20s memory-lifecycle animation, rendered to
 * a 2D context so it drives BOTH the live studio preview and the MP4 recorder
 * (one authored scene → live hero + downloadable video).
 *
 * Compositing: an internal WebGL ledger field (the frontier + claim-ticks) is
 * drawn under a Canvas2D typographic layer that tells the story — teach → a fact
 * changes → the strike + receipt → current truth. Colours are read from the CSS
 * palette so it always matches the site.
 */

export const HERO_MS = 21000;

const RW = 1920, RH = 1080; // internal render size (even → H.264-safe)

const VERT = `#version 300 es
in vec2 a; void main(){ gl_Position = vec4(a, 0.0, 1.0); }`;

const FRAG = `#version 300 es
precision highp float;
out vec4 frag;
uniform vec2 u_res; uniform float u_time; uniform float u_heat; uniform float u_strike;
uniform vec3 u_frontier; uniform vec3 u_live; uniform vec3 u_current; uniform vec3 u_stale; uniform vec3 u_conflict; uniform vec3 u_ink;
float hash(vec2 p){ p = fract(p*vec2(123.34,345.45)); p += dot(p,p+34.345); return fract(p.x*p.y); }
void main(){
  vec2 px = gl_FragCoord.xy; vec2 uv = px/u_res;
  vec3 col = u_ink;
  // faint ledger rules
  float pitch = 44.0; float d = abs(mod(px.y + u_time*5.0, pitch) - pitch*0.5);
  col += vec3(0.06) * exp(-d*d*0.5) * 0.5;
  // frontier line rising with heat
  float fy = mix(0.10,0.92,u_heat)*u_res.y; float fd = abs(px.y-fy);
  float front = exp(-fd*fd*0.0007); float shimmer = 0.6+0.4*sin(px.x*0.02+u_time*1.4);
  col += u_frontier * front * shimmer * 1.1;
  // claim-ticks below the frontier
  float cell=52.0; vec2 gid=floor(px/cell); vec2 gf=fract(px/cell)-0.5; float h=hash(gid);
  if(h>0.7){ float below=smoothstep(0.0,80.0,fy-px.y); float tw=0.55+0.45*sin(u_time*1.1+h*40.0);
    float dot=exp(-dot(gf,gf)*40.0); float s=hash(gid+7.1);
    vec3 c = mix(u_frontier,u_live,0.4)*0.5;
    if(s>0.93) c=u_conflict; else if(s>0.82) c=u_stale; else if(s>0.66) c=u_current;
    col += c * dot * below * tw * 0.85; }
  // strike ripple
  if(u_strike>0.001){ float sy=mix(0.92,0.10,1.0-u_strike)*u_res.y; float sd=abs(px.y-sy);
    col += u_stale * exp(-sd*sd*0.003) * u_strike * 1.4; }
  float vig = smoothstep(1.25,0.35,length(uv-0.5));
  frag = vec4(col*vig, 1.0);
}`;

type RGB = [number, number, number];
function css(name: string, fb: string): RGB {
  const v = (getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fb);
  const c = document.createElement('canvas').getContext('2d')!;
  c.fillStyle = v; c.fillRect(0, 0, 1, 1);
  const [r, g, b] = c.getImageData(0, 0, 1, 1).data;
  return [r / 255, g / 255, b / 255];
}
const hex = (c: RGB) => `rgb(${c.map((x) => Math.round(x * 255)).join(',')})`;

class Field {
  canvas = document.createElement('canvas');
  private gl: WebGL2RenderingContext | null;
  private U: Record<string, WebGLUniformLocation | null> = {};
  ok = false;
  constructor() {
    this.canvas.width = RW; this.canvas.height = RH;
    this.gl = this.canvas.getContext('webgl2', { antialias: false });
    if (!this.gl) return;
    const gl = this.gl;
    const mk = (t: number, s: string) => { const sh = gl.createShader(t)!; gl.shaderSource(sh, s); gl.compileShader(sh); return gl.getShaderParameter(sh, gl.COMPILE_STATUS) ? sh : null; };
    const vs = mk(gl.VERTEX_SHADER, VERT), fs = mk(gl.FRAGMENT_SHADER, FRAG);
    if (!vs || !fs) return;
    const p = gl.createProgram()!; gl.attachShader(p, vs); gl.attachShader(p, fs); gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) return;
    gl.useProgram(p);
    const b = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, b);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const loc = gl.getAttribLocation(p, 'a'); gl.enableVertexAttribArray(loc); gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
    for (const n of ['u_res', 'u_time', 'u_heat', 'u_strike', 'u_frontier', 'u_live', 'u_current', 'u_stale', 'u_conflict', 'u_ink']) this.U[n] = gl.getUniformLocation(p, n);
    gl.uniform2f(this.U.u_res!, RW, RH);
    gl.uniform3fv(this.U.u_frontier!, css('--frontier', '#5c9ce0'));
    gl.uniform3fv(this.U.u_live!, css('--live', '#2dd4bf'));
    gl.uniform3fv(this.U.u_current!, css('--current', '#4ade80'));
    gl.uniform3fv(this.U.u_stale!, css('--stale', '#fbbf24'));
    gl.uniform3fv(this.U.u_conflict!, css('--conflict', '#f87171'));
    gl.uniform3fv(this.U.u_ink!, css('--ink-void', '#08090b'));
    this.ok = true;
  }
  render(t: number, heat: number, strike: number) {
    if (!this.ok || !this.gl) return;
    const gl = this.gl;
    gl.uniform1f(this.U.u_time!, t); gl.uniform1f(this.U.u_heat!, heat); gl.uniform1f(this.U.u_strike!, strike);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }
}

// ── the timeline script: heat sweep + strike pulses + which beat is active ──
function script(t: number) {
  const heat = Math.min(1, t / (HERO_MS / 1000) * 1.05);
  // strike fires at the supersession climax (~9.5s), decays
  const strike = t > 9.5 ? Math.max(0, 1 - (t - 9.5) / 1.3) : 0;
  return { heat, strike };
}

const P = {
  ink: () => hex(css('--ink-void', '#08090b')),
  paper: () => hex(css('--paper', '#e7e5e4')),
  dim: () => hex(css('--paper-dim', '#a8a29e')),
  faint: () => 'rgba(168,162,158,0.55)',
  frontier: () => hex(css('--frontier', '#5c9ce0')),
  live: () => hex(css('--live', '#2dd4bf')),
  current: () => hex(css('--current', '#4ade80')),
  stale: () => hex(css('--stale', '#fbbf24')),
};

const SERIF = '"Instrument Serif", Georgia, serif';
const MONO = '"Space Mono", ui-monospace, monospace';

function fade(t: number, a: number, b: number, dur = 0.6) {
  if (t < a) return 0; if (t > b) return Math.max(0, 1 - (t - b) / dur);
  return Math.min(1, (t - a) / dur);
}

export function makeHero() {
  const field = new Field();
  return {
    fieldOk: field.ok,
    frame(ctx: OffscreenCanvasRenderingContext2D | CanvasRenderingContext2D, tMs: number, cw: number, ch: number) {
      const t = tMs / 1000;
      const s = script(t);
      field.render(t, s.heat, s.strike);
      ctx.fillStyle = P.ink(); ctx.fillRect(0, 0, cw, ch);
      if (field.ok) { (ctx as any).globalAlpha = 1; ctx.drawImage(field.canvas, 0, 0, cw, ch); }
      const k = cw / 1920; // scale factor
      ctx.textBaseline = 'alphabetic';

      // persistent brand + frontier counter
      ctx.globalAlpha = 0.9; ctx.fillStyle = P.faint(); ctx.font = `700 ${20 * k}px ${MONO}`;
      ctx.fillText('▍ texo · claim-chain memory', 80 * k, 90 * k);
      ctx.textAlign = 'right';
      ctx.fillStyle = P.live();
      ctx.fillText(`frontier ${String(Math.round(9 + s.heat * 38)).padStart(4, '0')}`, cw - 80 * k, 90 * k);
      ctx.textAlign = 'left';

      // Act I — the thesis
      let a = fade(t, 0.6, 8.5);
      if (a > 0) {
        ctx.globalAlpha = a; ctx.fillStyle = P.paper(); ctx.font = `400 ${92 * k}px ${SERIF}`;
        ctx.fillText('Memory that knows', 80 * k, ch * 0.42);
        ctx.fillStyle = P.live(); ctx.font = `italic 400 ${92 * k}px ${SERIF}`;
        ctx.fillText('when to stop believing things.', 80 * k, ch * 0.42 + 104 * k);
        ctx.globalAlpha = a * 0.8; ctx.fillStyle = P.dim(); ctx.font = `300 ${30 * k}px ${SERIF}`;
        ctx.fillText('Git tracks code diffs. texo tracks claim diffs.', 82 * k, ch * 0.42 + 170 * k);
      }

      // Act II — the supersession (claims + strike + receipt)
      a = fade(t, 8.6, 15.5);
      if (a > 0) {
        const x = 80 * k, y = ch * 0.4;
        ctx.globalAlpha = a; ctx.fillStyle = P.faint(); ctx.font = `700 ${20 * k}px ${MONO}`;
        ctx.fillText('THE SUPERSESSION', x, y - 46 * k);
        // old claim (struck after ~9.6s)
        ctx.fillStyle = t > 9.6 ? P.dim() : P.paper(); ctx.font = `400 ${52 * k}px ${SERIF}`;
        const old = 'Alice owns release approval.';
        ctx.fillText(old, x, y);
        if (t > 9.6) { const sw = Math.min(1, (t - 9.6) / 0.5) * ctx.measureText(old).width; ctx.strokeStyle = P.stale(); ctx.lineWidth = 3 * k; ctx.beginPath(); ctx.moveTo(x, y - 16 * k); ctx.lineTo(x + sw, y - 16 * k); ctx.stroke(); }
        // receipt
        const ra = fade(t, 10.4, 15.5);
        if (ra > 0) { ctx.globalAlpha = a * ra; ctx.fillStyle = P.faint(); ctx.font = `400 ${22 * k}px ${MONO}`; ctx.fillText('retired by  receipt ', x + 30 * k, y + 52 * k); ctx.fillStyle = P.live(); ctx.fillText('42abd965 · seq 15', x + 320 * k, y + 52 * k); }
        // new claim rises
        const na = fade(t, 11.0, 15.5);
        if (na > 0) { ctx.globalAlpha = a * na; ctx.fillStyle = P.paper(); ctx.font = `400 ${52 * k}px ${SERIF}`; ctx.fillText('Ben owns release approval now.', x, y + 120 * k); ctx.fillStyle = P.current(); ctx.font = `700 ${20 * k}px ${MONO}`; ctx.fillText('● CURRENT', x, y + 158 * k); }
      }

      // Act III — current truth
      a = fade(t, 15.6, 21);
      if (a > 0) {
        ctx.globalAlpha = a; ctx.fillStyle = P.paper(); ctx.font = `400 ${72 * k}px ${SERIF}`;
        ctx.fillText('Current truth, stale record,', 80 * k, ch * 0.44);
        ctx.fillText('open conflicts — with receipts.', 80 * k, ch * 0.44 + 84 * k);
        ctx.font = `700 ${22 * k}px ${MONO}`;
        ctx.fillStyle = P.current(); ctx.fillText('● current', 82 * k, ch * 0.44 + 150 * k);
        ctx.fillStyle = P.stale(); ctx.fillText('● stale', 300 * k, ch * 0.44 + 150 * k);
        ctx.fillStyle = P.frontier(); ctx.fillText('● conflict', 480 * k, ch * 0.44 + 150 * k);
      }
      ctx.globalAlpha = 1;
    },
  };
}
