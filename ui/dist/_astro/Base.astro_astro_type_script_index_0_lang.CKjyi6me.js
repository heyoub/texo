import{n as e,o as t}from"./boundary.CnnPgsOH.js";var n=[[0,`settling`],[.28,`accumulating`],[.62,`superseding`],[.9,`sealed`]];function r(e){let t=`settling`;for(let[r,i]of n)e>=r&&(t=i);return t}function i(){let n=document.documentElement,i=matchMedia(`(prefers-reduced-motion: reduce)`).matches,o=a(t(`scroll.progress`)),s=o,c=``,l=e=>{e!==c&&(c=e,n.setAttribute(`data-mood`,e),n.dispatchEvent(new CustomEvent(`texo:mood`,{detail:{mood:e},bubbles:!0})))};if(i){let i=()=>{let e=a(t(`scroll.progress`));n.style.setProperty(`--mem-heat`,e.toFixed(4)),n.style.setProperty(`--mem-frontier`,e.toFixed(4)),l(r(e))};e(`scroll.progress`,i),i();return}e(`scroll.progress`,()=>{o=a(t(`scroll.progress`))});let u=()=>{let e=o-s;Math.abs(e)>4e-4?(s+=e*.12,n.style.setProperty(`--mem-heat`,s.toFixed(4)),n.style.setProperty(`--mem-frontier`,s.toFixed(4)),l(r(s)),requestAnimationFrame(u)):(s=o,n.style.setProperty(`--mem-heat`,s.toFixed(4)),n.style.setProperty(`--mem-frontier`,s.toFixed(4)),l(r(s)),d=!1)},d=!1,f=()=>{d||(d=!0,requestAnimationFrame(u))};e(`scroll.progress`,f),f()}function a(e){return e===void 0||Number.isNaN(e)?0:Math.min(1,Math.max(0,e))}var o=`#version 300 es
in vec2 a; void main(){ gl_Position = vec4(a, 0.0, 1.0); }`,s=`#version 300 es
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
}`;function c(e,t,n){let r=e.createShader(t);return e.shaderSource(r,n),e.compileShader(r),e.getShaderParameter(r,e.COMPILE_STATUS)?r:(console.warn(`[ledger-field]`,e.getShaderInfoLog(r)),null)}function l(e){let t=getComputedStyle(document.documentElement).getPropertyValue(e).trim(),n=t.match(/^#?([0-9a-f]{6})$/i);if(n){let e=parseInt(n[1],16);return[(e>>16&255)/255,(e>>8&255)/255,(e&255)/255]}let r=document.createElement(`canvas`).getContext(`2d`);r.fillStyle=t||`#888`,r.fillRect(0,0,1,1);let[i,a,o]=r.getImageData(0,0,1,1).data;return[i/255,a/255,o/255]}function u(e){let n=document.documentElement,r=n.getAttribute(`data-czap-tier`)??`reactive`,i=n.getAttribute(`data-czap-motion`)??`animations`;if(matchMedia(`(prefers-reduced-motion: reduce)`).matches||i===`none`||r===`static`||r===`styled`)return null;let a=e.getContext(`webgl2`,{alpha:!0,premultipliedAlpha:!0,antialias:!1,powerPreference:`low-power`});if(!a)return null;let u=c(a,a.VERTEX_SHADER,o),d=c(a,a.FRAGMENT_SHADER,s);if(!u||!d)return null;let f=a.createProgram();if(a.attachShader(f,u),a.attachShader(f,d),a.linkProgram(f),!a.getProgramParameter(f,a.LINK_STATUS))return console.warn(`[ledger-field]`,a.getProgramInfoLog(f)),null;a.useProgram(f);let p=a.createBuffer();a.bindBuffer(a.ARRAY_BUFFER,p),a.bufferData(a.ARRAY_BUFFER,new Float32Array([-1,-1,3,-1,-1,3]),a.STATIC_DRAW);let m=a.getAttribLocation(f,`a`);a.enableVertexAttribArray(m),a.vertexAttribPointer(m,2,a.FLOAT,!1,0,0),a.enable(a.BLEND),a.blendFunc(a.ONE,a.ONE_MINUS_SRC_ALPHA);let h={res:a.getUniformLocation(f,`u_res`),time:a.getUniformLocation(f,`u_time`),heat:a.getUniformLocation(f,`u_heat`),strike:a.getUniformLocation(f,`u_strike`),frontier:a.getUniformLocation(f,`u_frontier`),live:a.getUniformLocation(f,`u_live`),current:a.getUniformLocation(f,`u_current`),stale:a.getUniformLocation(f,`u_stale`),conflict:a.getUniformLocation(f,`u_conflict`)};a.uniform3fv(h.frontier,l(`--frontier`)),a.uniform3fv(h.live,l(`--live`)),a.uniform3fv(h.current,l(`--current`)),a.uniform3fv(h.stale,l(`--stale`)),a.uniform3fv(h.conflict,l(`--conflict`));let g=Math.min(devicePixelRatio||1,1.75);function _(){let t=Math.floor(innerWidth*g),n=Math.floor(innerHeight*g);e.width=t,e.height=n,a.viewport(0,0,t,n),a.uniform2f(h.res,t,n)}_(),addEventListener(`resize`,_,{passive:!0});let v=0,y=0,b=0,x=0,S=!0;document.addEventListener(`texo:supersede`,()=>{y=1});function C(e){if(!S)return;b||=e;let n=(e-b)/1e3,r=Math.min(1,Math.max(0,t(`scroll.progress`)??0));v+=(r-v)*.06,y*=.955,a.uniform1f(h.time,n),a.uniform1f(h.heat,v),a.uniform1f(h.strike,y<.002?0:y),a.drawArrays(a.TRIANGLES,0,3),x=requestAnimationFrame(C)}x=requestAnimationFrame(C);let w=()=>{document.hidden?(S=!1,cancelAnimationFrame(x)):S||(S=!0,b=0,x=requestAnimationFrame(C))};return document.addEventListener(`visibilitychange`,w),n.setAttribute(`data-field`,`on`),()=>{S=!1,cancelAnimationFrame(x),removeEventListener(`resize`,_),document.removeEventListener(`visibilitychange`,w),n.removeAttribute(`data-field`)}}var d=matchMedia(`(prefers-reduced-motion: reduce)`).matches,f=document.querySelectorAll(`.reveal`);if(d)f.forEach(e=>e.classList.add(`is-visible`));else{let e=new IntersectionObserver(t=>{for(let n of t)n.isIntersecting&&(n.target.classList.add(`is-visible`),e.unobserve(n.target))},{rootMargin:`0px 0px -80px 0px`,threshold:.05});f.forEach(t=>e.observe(t))}i();var p=document.getElementById(`ledger-canvas`);p&&u(p);