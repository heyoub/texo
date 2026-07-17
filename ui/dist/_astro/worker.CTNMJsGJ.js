import{o as e,t}from"./variants.nROX2fDU.js";import{a as n,r}from"./brands.f6mJBjQo.js";import{n as i,r as a}from"./wasm-dispatch.BM4xd22M.js";import{n as o,t as s}from"./diagnostics.DGKHtgqK.js";import{t as c}from"./projection.FtIQGdek.js";import{a as l,i as u,n as d,o as f,r as p,s as m,t as h}from"./boundary.CnnPgsOH.js";import{t as g}from"./plan.ClalsVhr.js";import{r as _}from"./page.BXd3Qh7l.js";var v=e=>e,y=-1/0;function b(t,n){let r=new Map,i=[],a=new Float64Array(n);a.fill(y);let o={name:t,capacity:n,_dense:!0,entityToIndex:r,indexToEntity:i,data:a,count:0,get(e){let t=r.get(e);if(t!==void 0)return a[t]},set(s,c){let l=r.get(s);if(l!==void 0){a[l]=c;return}if(o.count>=n)throw e(`Part.dense`,`store "${t}" at capacity (${n}). Cannot add entity ${s}. Create the store with a larger capacity (Part.dense(name, n)) or remove entities before adding.`);l=o.count,r.set(s,l),i[l]=s,a[l]=c,o.count++},has(e){return r.has(e)},delete(e){let t=r.get(e);if(t===void 0)return!1;let n=o.count-1;if(t!==n){let e=i[n];a[t]=a[n],i[t]=e,r.set(e,t)}return a[n]=y,i.length=n,r.delete(e),o.count--,!0},reset(){r.clear(),i.length=0,a.fill(y),o.count=0},view(){return a.subarray(0,o.count)},entities(){return i}};return o}function x(e,t){return b(e,t)}var S={dense:x},C=128;function w(e){return g.make(e).step(`compute-discrete`,{type:`noop`},{phase:`compute-discrete`}).step(`compute-blend`,{type:`noop`},{phase:`compute-blend`}).step(`emit-css`,{type:`noop`},{phase:`emit-css`}).step(`emit-glsl`,{type:`noop`},{phase:`emit-glsl`}).step(`emit-wgsl`,{type:`noop`},{phase:`emit-wgsl`}).step(`emit-aria`,{type:`noop`},{phase:`emit-aria`}).seq(`step-1`,`step-2`).par(`step-2`,`step-3`).par(`step-2`,`step-4`).par(`step-2`,`step-5`).par(`step-2`,`step-6`).build()}function T(e){let t=g.topoSort(e).sorted,n=new Map(e.steps.map(e=>[e.id,e]));return t.map(e=>n.get(e)?.metadata?.phase).filter(e=>typeof e==`string`)}var E=w(`czap-runtime`),D=T(E);function ee(e){let t=e?.name??`czap-runtime`,n=t===E.name?E:{...E,name:t},r=D,i=S.dense(`state-index`,e?.capacity??C),a=S.dense(`dirty-epoch`,e?.capacity??C),o=new Map,s=0,c=e=>o.get(e);return{plan:n,phases:r,stores:{stateIndex:i,dirtyEpoch:a},reset(e){o.clear(),s=0,i.reset(),a.reset();for(let t of e??[])this.registerQuantizer(t.name,t.states)},registerQuantizer(e,t){let n=c(e);if(n)return n.entityId;let r=v(`runtime-${++s}`),l=Object.create(null);for(let e=0;e<t.length;e++)l[t[e]]=e;return o.set(e,{entityId:r,stateLookup:l}),i.set(r,0),a.set(r,1),r},removeQuantizer(e){let t=c(e);t&&(o.delete(e),i.delete(t.entityId),a.delete(t.entityId))},hasQuantizer(e){return o.has(e)},setState(e,t){let n=c(e);n&&i.set(n.entityId,n.stateLookup[t]??0)},applyState(e,t){let n=c(e);if(!n)return 0;let r=n.stateLookup[t]??0;return i.set(n.entityId,r),r},getStateIndex(e){let t=c(e);return t?i.get(t.entityId):0},markDirty(e){let t=c(e);t&&a.set(t.entityId,a.get(t.entityId)+1)},getDirtyEpoch(e){let t=c(e);return t?a.get(t.entityId):0},registeredNames(){return Array.from(o.keys())}}}var te={create:ee};function ne(e,t,n){return{type:e,states:t,ack:n}}var re=`
"use strict";

// ---------------------------------------------------------------------------
// Simplified compositor state inside the worker
// ---------------------------------------------------------------------------

/** @type {Map<string, { id: string; states: string[]; thresholds: number[]; currentState: string; currentGeneration: number; cssKey: string|null; glslKey: string|null; ariaKey: string|null; oneHotWeights: Record<string, Record<string, number>>|null; _keysResolved: boolean }>} */
const quantizers = new Map();

/** @type {Map<string, Record<string, number>>} */
const blendOverrides = new Map();

/** @type {Set<string>} */
const dirtyNames = new Set();

const MS_PER_SEC = 1000;

/** @type {number} */
let lastComputeTime = 0;
let frameCount = 0;
let fpsAccum = 0;
let currentFps = 0;

function removeQuantizer(name) {
  quantizers.delete(name);
  blendOverrides.delete(name);
  dirtyNames.delete(name);
}

function evaluateQuantizer(name, value) {
  const q = quantizers.get(name);
  if (q) {
    const newState = evaluateThresholds(q.thresholds, q.states, value);
    if (newState !== q.currentState) {
      q.currentState = newState;
      dirtyNames.add(name);
    }
  }
}

function setBlendWeights(name, weights) {
  blendOverrides.set(name, weights);
  dirtyNames.add(name);
}

function applyResolvedStateEntry(entry) {
  const q = quantizers.get(entry.name);
  if (!q) {
    return;
  }

  const nextGeneration = typeof entry.generation === "number" ? entry.generation : q.currentGeneration;
  const changed = entry.state !== q.currentState || nextGeneration !== q.currentGeneration;
  q.currentState = entry.state;
  q.currentGeneration = nextGeneration;
  if (changed) {
    dirtyNames.add(entry.name);
  }
}

function applyUpdate(update) {
  switch (update.type) {
    case "remove-quantizer":
      removeQuantizer(update.name);
      break;
    case "evaluate":
      evaluateQuantizer(update.name, update.value);
      break;
    case "set-blend":
      setBlendWeights(update.name, update.weights);
      break;
  }
}

function registerQuantizer(registration) {
  const initialState =
    typeof registration.initialState === "string"
      ? registration.initialState
      : registration.states[0] || "";
  const thresholdsRaw = registration.thresholds;
  const thresholds = thresholdsRaw instanceof Float64Array
    ? Array.from(thresholdsRaw)
    : Array.from(thresholdsRaw);
  quantizers.set(registration.name, {
    id: registration.boundaryId,
    states: Array.from(registration.states),
    thresholds: thresholds,
    currentState: initialState,
    currentGeneration: 0,
    cssKey: null,
    glslKey: null,
    ariaKey: null,
    oneHotWeights: null,
    _keysResolved: false,
  });
  if (registration.blendWeights && typeof registration.blendWeights === "object") {
    blendOverrides.set(registration.name, registration.blendWeights);
  } else {
    blendOverrides.delete(registration.name);
  }
  dirtyNames.add(registration.name);
}

${c}

function resolveOutputKeys(q, name) {
  if (q._keysResolved) return;
  const keys = projectionKeys(name);
  q.cssKey = keys.cssKey;
  q.glslKey = keys.glslKey;
  q.ariaKey = keys.ariaKey;
  q.oneHotWeights = Object.fromEntries(
    q.states.map((activeState) => [
      activeState,
      Object.fromEntries(
        q.states.map((stateName) => [stateName, stateName === activeState ? 1 : 0]),
      ),
    ]),
  );
  q._keysResolved = true;
}

function resetWorkerState() {
  quantizers.clear();
  blendOverrides.clear();
  dirtyNames.clear();
}

${i}

/**
 * Build a CompositeState from the current quantizer state.
 * @returns {{ discrete: Record<string, string>; blend: Record<string, Record<string, number>>; outputs: { css: Record<string, number|string>; glsl: Record<string, number>; wgsl: Record<string, number>; aria: Record<string, string> } }}
 */
function compute() {
  const now = typeof performance !== "undefined" ? performance.now() : Date.now();

  const discrete = {};
  const blend = {};
  const css = {};
  const glsl = {};
  // WGSL channel: the live state index is emitted below into the single fixed
  // state_index struct field (slot 0), mirroring the host emit-wgsl so off-thread
  // WGSL shaders driven by client:worker receive the same crossing as client:gpu.
  const wgsl = {};
  const aria = {};
  const resolvedStateGenerations = {};

  // Only recompute dirty quantizers if we have a dirty set,
  // otherwise recompute all (initial case or fallback).
  const names = dirtyNames.size > 0
    ? Array.from(dirtyNames)
    : Array.from(quantizers.keys());

  for (const name of names) {
    const q = quantizers.get(name);
    if (!q) continue;

    // Lazily resolve output keys on first compute
    resolveOutputKeys(q, name);

    const stateStr = q.currentState;
    discrete[name] = stateStr;
    resolvedStateGenerations[name] = q.currentGeneration;

    // Blend weights
      const override = blendOverrides.get(name);
      if (override !== undefined) {
        blend[name] = override;
      } else {
        blend[name] = q.oneHotWeights[stateStr] || {};
      }

      // CSS output
      css[q.cssKey] = stateStr;

    // GLSL output: index of current state
    let stateIndex = 0;
    for (let i = 0; i < q.states.length; i++) {
      if (q.states[i] === stateStr) {
        stateIndex = i;
        break;
      }
    }
      glsl[q.glslKey] = stateIndex;

      // WGSL output: the live state index goes into the single fixed state_index
      // struct field (slot 0), matching the host emit-wgsl + the wgpu runtime.
      wgsl['state_index'] = stateIndex;

      // ARIA output
      aria[q.ariaKey] = stateStr;
  }

  dirtyNames.clear();

  // Metrics
  if (lastComputeTime > 0) {
    const dt = now - lastComputeTime;
    frameCount++;
    fpsAccum += dt;
    if (fpsAccum >= MS_PER_SEC) {
      currentFps = Math.round((frameCount * MS_PER_SEC) / fpsAccum);
      frameCount = 0;
      fpsAccum -= MS_PER_SEC;

      self.postMessage({
        type: "metrics",
        fps: currentFps,
        budgetUsed: dt,
      });
    }
  }
  lastComputeTime = now;

  return { discrete, blend, outputs: { css, glsl, wgsl, aria }, resolvedStateGenerations };
}

// ---------------------------------------------------------------------------
// Message handler
// ---------------------------------------------------------------------------

self.addEventListener("message", function (e) {
  const msg = e.data;
  if (!msg || typeof msg.type !== "string") return;

  switch (msg.type) {
    case "init": {
      // Reset state on init
      resetWorkerState();
      self.postMessage({ type: "ready" });
      break;
    }

    case "add-quantizer": {
      registerQuantizer(msg);
      break;
    }

    case "bootstrap-quantizers": {
      for (const registration of msg.registrations) {
        registerQuantizer(registration);
      }
      break;
    }

    case "startup-compute": {
      resetWorkerState();
      const packet = msg.packet ?? { registrations: [], updates: [] };
      for (const registration of packet.registrations) {
        registerQuantizer(registration);
      }
      for (const update of packet.updates) {
        applyUpdate(update);
      }
      try {
        const state = compute();
        self.postMessage({ type: "state", state: state, resolvedStateGenerations: state.resolvedStateGenerations });
      } catch (err) {
        self.postMessage({
          type: "error",
          code: "startup-compute-failed",
          message: err instanceof Error ? err.message : String(err),
          hint: "compute() threw while applying the startup packet — check the registrations and updates in the startup-compute message.",
          context: msg.type,
        });
      }
      break;
    }

    case "bootstrap-resolved-state": {
      for (const entry of msg.states) {
        applyResolvedStateEntry(entry);
      }
      if (msg.ack === true) {
        self.postMessage({
          type: "resolved-state-ack",
          generation: typeof msg.states[0]?.generation === "number" ? msg.states[0].generation : 0,
          states: msg.states.map((entry) => ({ name: entry.name, state: entry.state })),
          additionalOutputsChanged: false,
        });
      }
      break;
    }

    case "apply-resolved-state": {
      for (const entry of msg.states) {
        applyResolvedStateEntry(entry);
      }
      if (msg.ack === true) {
        self.postMessage({
          type: "resolved-state-ack",
          generation: typeof msg.states[0]?.generation === "number" ? msg.states[0].generation : 0,
          states: msg.states.map((entry) => ({ name: entry.name, state: entry.state })),
          additionalOutputsChanged: false,
        });
      }
      break;
    }

    case "remove-quantizer": {
      removeQuantizer(msg.name);
      break;
    }

    case "evaluate": {
      evaluateQuantizer(msg.name, msg.value);
      break;
    }

    case "set-blend": {
      setBlendWeights(msg.name, msg.weights);
      break;
    }

    case "apply-updates": {
      for (const update of msg.updates) {
        applyUpdate(update);
      }
      break;
    }

    case "warm-reset": {
      blendOverrides.clear();
      dirtyNames.clear();
      for (const quantizer of quantizers.values()) {
        quantizer.currentState = quantizer.states[0] || "";
      }
      break;
    }

    case "compute": {
      try {
        const state = compute();
        self.postMessage({ type: "state", state: state, resolvedStateGenerations: state.resolvedStateGenerations });
      } catch (err) {
        self.postMessage({
          type: "error",
          code: "compute-failed",
          message: err instanceof Error ? err.message : String(err),
          hint: "compute() threw — check the quantizer registrations and the updates sent since the last compute.",
          context: msg.type,
        });
      }
      break;
    }

    case "dispose": {
      resetWorkerState();
      self.close();
      break;
    }
  }
});
`;function O(e,t){if(e.length!==t.length)return!1;for(let n=0;n<e.length;n++)if(e[n]!==t[n])return!1;return!0}function k(e,t){if(e===t)return!0;if(!e||!t)return e===t;let n=Object.keys(e),r=Object.keys(t);if(n.length!==r.length)return!1;for(let r of n)if(e[r]!==t[r])return!1;return!0}function A(e){return e.map(e=>({name:e.name,states:e.states}))}function ie(e,t=[]){return{bootstrapMode:e,registrations:new Map(t.map(e=>[e.name,e])),registrationList:t.length>0?[...t]:[],runtimeSeedList:t.length>0?A(t):[],updates:[],runtimeSeedDirty:!1}}function ae(e){return{bootstrapMode:e.bootstrapMode,registrations:j(e),updates:e.updates}}function j(e){return e.registrationList===null&&(e.registrationList=Array.from(e.registrations.values())),e.registrationList}function oe(e){return e.runtimeSeedList!==null&&!e.runtimeSeedDirty?e.runtimeSeedList:(e.runtimeSeedList=A(j(e)),e.runtimeSeedDirty=!1,e.runtimeSeedList)}function M(e,t,n=!0){e.registrations.set(t.name,t),e.registrationList=null,n&&(e.runtimeSeedList=null,e.runtimeSeedDirty=!0)}function se(e,t){e.registrations.delete(t),e.registrationList=null,e.runtimeSeedList=null,e.runtimeSeedDirty=!0}function ce(e,t){e.updates.push(t)}function N(e,t){if(e.updates.length===0)return;let n=e.updates.filter(t);n.length!==e.updates.length&&(e.updates=n)}function le(e,t,r){let i=e.registrations.get(t.name),a=r===i.states[0]?(()=>{let{initialState:e,...t}=i;return t})():{...i,initialState:n(r)},o=`initialState`in a?a.initialState:void 0;(i.boundaryId!==a.boundaryId||!O(i.states,a.states)||!O(i.thresholds,a.thresholds)||i.initialState!==o||!k(i.blendWeights,a.blendWeights))&&M(e,a,!1),N(e,e=>!(e.type===`evaluate`&&e.name===t.name))}function ue(e,t,n){let r=e.registrations.get(t);return r?(k(r.blendWeights,n)||M(e,{...r,blendWeights:n},!1),N(e,e=>!(e.type===`set-blend`&&e.name===t)),!0):!1}function de(e,t){if(se(e,t),e.updates.length===0)return;let n=e.updates.filter(e=>e.name!==t);n.length!==e.updates.length&&(e.updates=n)}function P(e){e.registrations.clear(),e.registrationList=[],e.runtimeSeedList=[],e.updates=[],e.runtimeSeedDirty=!1}var F=null,I=null,L=!1,R=null;function z(){return o.now()*1e6}function B(e,t,n){let r=e?.recordDiagnosticStage;r?.(t,n)}function fe(e,t){let n=e?.onResolvedStateSettled;n?.(t)}function V(e,t){if(!O(e.registeredNames(),t.map(e=>e.name)))return!1;for(let n of t)if(!e.hasQuantizer(n.name))return!1;return!0}function H(){F&&(URL.revokeObjectURL(F),F=null,I=null)}function U(){R?.worker.terminate(),R=null}function pe(){if(typeof globalThis>`u`||!(`process`in globalThis))return null;let e=globalThis.process;return typeof e!=`object`||!e?null:e}function me(){if(L)return;L=!0;let e=()=>{U(),H()};if(typeof globalThis.addEventListener==`function`){globalThis.addEventListener(`pagehide`,e,{once:!0});return}let t=pe();t!==null&&typeof t.once==`function`&&t.once(`exit`,e)}function he(){return F&&I===URL.createObjectURL?F:(F&&H(),F=URL.createObjectURL(new Blob([re],{type:`application/javascript`})),I=URL.createObjectURL,me(),F)}function ge(){let e=he();return new Worker(e,{type:`classic`,name:`czap-compositor`})}function _e(e){return te.create({capacity:e,name:`czap-worker-runtime`})}function ve(e,t){R&&(R.workerConstructor!==Worker||R.createObjectUrl!==URL.createObjectURL||R.capacity!==e)&&U();let n=z(),r=R;R=null;let i=r?.worker??ge();t?.recordStage(`claim-or-create`,z()-n);let a=z(),o=r?.runtime??_e(e),s=r?.bootstrapSnapshot??[];if(r){let e=z();o.reset(),B(t,`coordinator-reset-or-create:runtime-reset-reuse`,z()-e)}return t?.recordStage(`coordinator-reset-or-create`,z()-a),{worker:i,runtime:o,bootstrapSnapshot:s}}function ye(e){if(!R&&typeof Worker<`u`&&Worker===e.worker.constructor&&typeof URL.createObjectURL==`function`&&URL.createObjectURL===I){R={...e,workerConstructor:Worker,createObjectUrl:URL.createObjectURL};return}W(e.worker,{type:`dispose`}),e.worker.terminate()}function W(e,t,n){e.postMessage(t,n??[])}function be(e){let t=new Float64Array(e.thresholds);return{registration:{...e,thresholds:t},buffer:t.buffer}}function G(e,t){return e?e.boundaryId===t.boundaryId&&O(e.states,t.states)&&O(e.thresholds,t.thresholds):!1}function xe(e,t){let n=a(e.thresholds,t);return e.states[n]??e.states[0]}function Se(e){return e.states.map(t=>({name:t.name,state:n(t.state),generation:e.generation}))}function Ce(){return{_tag:`startup`}}function K(e){return e._tag===`startup`}function q(e={}){return{_tag:`steady`,firstStateDispatchCompletedNs:e.firstStateDispatchCompletedNs??null,firstStatePending:e.firstStatePending??!1,resolvedStateDispatchCompletedNs:null,resolvedStateAckPending:!1}}function we(e){let{worker:t,runtime:n,capacity:r,bootstrapSnapshot:i,startupTelemetry:a}=e;return{worker:t,runtime:n,capacity:r,startupTelemetry:a,snapshotByName:new Map(i.map(e=>[e.name,e])),activeRegistrations:new Map,confirmedSnapshotNames:new Set,preparedRegistrationCache:new Map,startupPacket:ie(i.length>0?`warm-snapshot`:`cold`,i),steadyStatePendingUpdates:[],flushScheduled:!1,mode:Ce(),stateListeners:new Set,resolvedStateAckListeners:new Set,metricsListeners:new Set,lastMetrics:null,lastWorkerError:null}}function Te(e,t){let n=e.preparedRegistrationCache.get(t.name);if(n&&n.source===t&&n.buffer.byteLength>0)return n;let{registration:r,buffer:i}=be(t),a={source:t,transferRegistration:r,buffer:i};return e.preparedRegistrationCache.set(t.name,a),a}function J(e,t){let n=[];return{registrations:t.map(t=>{let r=Te(e,t);return e.preparedRegistrationCache.delete(t.name),n.push(r.buffer),r.transferRegistration}),buffers:n}}function Y(e){if(e.flushScheduled=!1,e.steadyStatePendingUpdates.length===0)return;let t=e.steadyStatePendingUpdates;e.steadyStatePendingUpdates=[],W(e.worker,{type:`apply-updates`,updates:t})}function X(e,t){if(K(e.mode)){ce(e.startupPacket,t);return}e.steadyStatePendingUpdates.push(t),!e.flushScheduled&&(e.flushScheduled=!0,queueMicrotask(()=>Y(e)))}function Z(e){e.startupPacket.bootstrapMode===`warm-snapshot`&&(e.startupPacket.bootstrapMode=`rebuild`)}function Ee(e,t){for(let n of t)e.runtime.markDirty(n.name),e.runtime.applyState(n.name,n.state)}function De(e){if(!K(e.mode)){Y(e);return}e.mode=q(),e.flushScheduled=!1,e.steadyStatePendingUpdates=[];let t=Array.from(e.activeRegistrations.values());if(e.startupPacket.bootstrapMode!==`cold`&&W(e.worker,{type:`init`}),t.length>0){let{registrations:n,buffers:r}=J(e,t);W(e.worker,{type:`bootstrap-quantizers`,registrations:n},r)}P(e.startupPacket)}function Oe(e,t,n){if(n.length===0)return;De(e),Ee(e,n);let r=e.resolvedStateAckListeners.size>0||e.startupTelemetry!==void 0,i=z();W(e.worker,ne(t,n,r));let a=z();e.mode._tag===`steady`&&(e.mode.resolvedStateDispatchCompletedNs=a,e.mode.resolvedStateAckPending=r),B(e.startupTelemetry,`request-compute:dispatch-send`,a-i),B(e.startupTelemetry,`request-compute:packet-finalize`,0),B(e.startupTelemetry,`request-compute:post-send-bookkeeping`,0)}function ke(e,t,r){let i=typeof t==`string`?t:t.input,a=typeof t==`string`?{...r,states:r.states.map(e=>n(e))}:{...t,states:t.states.map(e=>n(e))},o={name:i,boundaryId:a.id,states:a.states,thresholds:a.thresholds};if(G(e.activeRegistrations.get(i),o)){e.startupPacket.bootstrapMode===`warm-snapshot`&&e.snapshotByName.has(i)&&e.confirmedSnapshotNames.add(i);return}e.preparedRegistrationCache.delete(i),e.activeRegistrations.set(i,o);let s=e.snapshotByName.get(i),c=G(s,o);if(e.runtime.hasQuantizer(i)&&!c&&e.runtime.removeQuantizer(i),e.runtime.hasQuantizer(i)||e.runtime.registerQuantizer(i,a.states),e.startupPacket.bootstrapMode===`warm-snapshot`&&c){e.confirmedSnapshotNames.add(i);return}if(e.confirmedSnapshotNames.delete(i),(s||e.startupPacket.bootstrapMode===`warm-snapshot`)&&Z(e),K(e.mode)){M(e.startupPacket,o);return}let{registrations:l,buffers:u}=J(e,[o]);W(e.worker,{type:`add-quantizer`,...l[0]},u)}function Ae(e,t){if(e.preparedRegistrationCache.delete(t),e.activeRegistrations.delete(t),e.confirmedSnapshotNames.delete(t),e.runtime.removeQuantizer(t),e.snapshotByName.has(t)&&Z(e),K(e.mode)){de(e.startupPacket,t);return}X(e,{type:`remove-quantizer`,name:t})}function je(e,t,n){if(K(e.mode)&&e.snapshotByName.has(t)&&!e.confirmedSnapshotNames.has(t)&&Z(e),K(e.mode)){let r=e.activeRegistrations.get(t);if(r){let i=xe(r,n);i!==r.states[0]&&e.confirmedSnapshotNames.delete(t),le(e.startupPacket,r,i),e.runtime.markDirty(t);return}}e.runtime.markDirty(t),X(e,{type:`evaluate`,name:t,value:n})}function Me(e,t,n){if(K(e.mode)&&e.snapshotByName.has(t)&&!e.confirmedSnapshotNames.has(t)&&Z(e),K(e.mode)&&ue(e.startupPacket,t,n)){e.confirmedSnapshotNames.delete(t),e.runtime.markDirty(t);return}e.runtime.markDirty(t),X(e,{type:`set-blend`,name:t,weights:n})}function Ne(e,t){Oe(e,`bootstrap-resolved-state`,t)}function Pe(e,t){Oe(e,`apply-resolved-state`,t)}function Fe(e){if(!K(e.mode)){Y(e),W(e.worker,{type:`compute`});return}let t=oe(e.startupPacket);if(e.startupPacket.bootstrapMode===`warm-snapshot`&&e.activeRegistrations.size===e.snapshotByName.size&&e.confirmedSnapshotNames.size===e.snapshotByName.size&&V(e.runtime,t)){let t=z();W(e.worker,{type:`warm-reset`}),W(e.worker,{type:`compute`});let n=z();B(e.startupTelemetry,`request-compute:dispatch-send`,n-t),B(e.startupTelemetry,`request-compute:packet-finalize`,0),B(e.startupTelemetry,`request-compute:post-send-bookkeeping`,0),e.mode=q({firstStateDispatchCompletedNs:n,firstStatePending:!0});return}let n=z(),r=ae(e.startupPacket);e.startupPacket.bootstrapMode===`rebuild`&&(V(e.runtime,t)||e.runtime.reset(t));let i=z();B(e.startupTelemetry,`request-compute:packet-finalize`,i-n),e.flushScheduled=!1;let{registrations:a,buffers:o}=J(e,r.registrations),s={...r,registrations:a},c=z();W(e.worker,{type:`startup-compute`,packet:s},o);let l=z();B(e.startupTelemetry,`request-compute:dispatch-send`,l-c),B(e.startupTelemetry,`request-compute:post-send-bookkeeping`,0),e.mode=q({firstStateDispatchCompletedNs:l,firstStatePending:!0})}function Ie(e,t){return e.stateListeners.add(t),()=>{e.stateListeners.delete(t)}}function Le(e,t){return e.resolvedStateAckListeners.add(t),()=>{e.resolvedStateAckListeners.delete(t)}}function Re(e,t){return e.metricsListeners.add(t),()=>{e.metricsListeners.delete(t)}}function ze(e){P(e.startupPacket),e.steadyStatePendingUpdates=[],e.flushScheduled=!1,e.stateListeners.clear(),e.resolvedStateAckListeners.clear(),e.metricsListeners.clear(),e.preparedRegistrationCache.clear(),e.lastMetrics=null,e.lastWorkerError=null}function Be(e,t){switch(t.type){case`ready`:return[];case`state`:{let n=[],r=e.mode._tag===`steady`&&e.mode.firstStatePending?e.mode:null,i=z();r&&n.push({_tag:`diagnostic-stage`,stage:`state-delivery:message-receipt`,durationNs:i-r.firstStateDispatchCompletedNs});for(let[n,r]of Object.entries(t.state.discrete??{}))e.runtime.applyState(n,r);let a=z();return r&&n.push({_tag:`diagnostic-stage`,stage:`state-delivery:callback-queue-turn`,durationNs:a-i}),n.push({_tag:`deliver-state`,state:{...t.state,resolvedStateGenerations:t.resolvedStateGenerations}}),r&&(n.push({_tag:`diagnostic-stage`,stage:`state-delivery:host-callback-delivery`,durationNs:z()-a}),r.firstStatePending=!1,r.firstStateDispatchCompletedNs=null),n}case`resolved-state-ack`:{if(!(e.mode._tag===`steady`&&e.mode.resolvedStateAckPending&&e.mode.resolvedStateDispatchCompletedNs!==null))return[{_tag:`resolved-state-settled`,ack:t}];let n=[],r=e.mode._tag===`steady`?e.mode.resolvedStateDispatchCompletedNs:0,i=z();n.push({_tag:`diagnostic-stage`,stage:`state-delivery:message-receipt`,durationNs:i-r});let a=z();return n.push({_tag:`diagnostic-stage`,stage:`state-delivery:callback-queue-turn`,durationNs:a-i}),n.push({_tag:`resolved-state-settled`,ack:t}),e.resolvedStateAckListeners.size>0?(n.push({_tag:`deliver-ack`,ack:t}),n.push({_tag:`diagnostic-stage`,stage:`state-delivery:host-callback-delivery`,durationNs:z()-a})):n.push({_tag:`diagnostic-stage`,stage:`state-delivery:host-callback-delivery`,durationNs:0}),e.mode._tag===`steady`&&(e.mode.resolvedStateAckPending=!1,e.mode.resolvedStateDispatchCompletedNs=null),n}case`metrics`:{let n={type:`metrics`,fps:t.fps,budgetUsed:t.budgetUsed};return e.lastMetrics=n,[{_tag:`deliver-metrics`,metrics:n}]}case`error`:return e.lastWorkerError=t.message,[{_tag:`worker-error`,code:t.code,message:t.message,hint:t.hint,context:t.context}];default:return[]}}function Ve(e,t){for(let n of t)switch(n._tag){case`diagnostic-stage`:B(e.startupTelemetry,n.stage,n.durationNs);break;case`deliver-state`:for(let t of e.stateListeners)t(n.state);break;case`resolved-state-settled`:fe(e.startupTelemetry,Se(n.ack));break;case`deliver-ack`:for(let t of e.resolvedStateAckListeners)t(n.ack);break;case`deliver-metrics`:for(let t of e.metricsListeners)t(n.metrics);break;case`worker-error`:s.error({source:`czap/worker.compositor-worker`,code:`worker-message-error`,message:n.context===void 0?`Compositor worker reported an error.`:`Compositor worker failed while handling "${n.context}". Most often a registration whose thresholds do not line up with its states (thresholds[i] is the lower bound of states[i]).`,detail:{code:n.code,message:n.message,hint:n.hint,context:n.context}});break}}function He(e,t){let n=e?.poolCapacity??64,{worker:r,runtime:i,bootstrapSnapshot:a}=ve(n,t),o=we({worker:r,runtime:i,capacity:n,bootstrapSnapshot:a,startupTelemetry:t}),c=e=>{let t=e.data;!t||typeof t.type!=`string`||Ve(o,Be(o,t))},l=e=>{s.error({source:`czap/worker.compositor-worker`,code:`worker-unhandled-error`,message:`Compositor worker raised an unhandled error (often the Blob-URL worker being blocked by a strict CSP — allow worker-src blob:). Detail: ${e.message}`,detail:e.message})},u=z();return r.addEventListener(`message`,c),r.addEventListener(`error`,l),t?.recordStage(`listener-bind`,z()-u),o.startupPacket.bootstrapMode===`cold`&&W(r,{type:`init`}),{get worker(){return r},get runtime(){return i},addQuantizer(e,t){ke(o,e,t)},removeQuantizer(e){Ae(o,e)},evaluate(e,t){je(o,e,t)},setBlendWeights(e,t){Me(o,e,t)},bootstrapResolvedState(e){Ne(o,e)},applyResolvedState(e){Pe(o,e)},requestCompute(){Fe(o)},onState(e){return Ie(o,e)},onResolvedStateAck(e){return Le(o,e)},onMetrics(e){return Re(o,e)},dispose(){ze(o),typeof r.removeEventListener==`function`&&(r.removeEventListener(`message`,c),r.removeEventListener(`error`,l)),ye({worker:r,runtime:i,capacity:n,bootstrapSnapshot:Array.from(o.activeRegistrations.values())})}}}var Ue={create:He},We=`
"use strict";

/** @type {OffscreenCanvas | null} */
let canvas = null;

/** @type {OffscreenCanvasRenderingContext2D | null} */
let ctx = null;

/** @type {boolean} */
let rendering = false;

/** @type {boolean} */
let stopRequested = false;

/**
 * Wall-clock frame pacing target (frames per second) received in the
 * init message. 0 means unpaced: the render loop free-runs at maximum
 * speed (the worker-local default).
 * @type {number}
 */
let targetFps = 0;

// ---------------------------------------------------------------------------
// Simplified inline compositor (mirrors compositor-worker / Boundary.evaluate)
// ---------------------------------------------------------------------------

/** @type {Map<string, { id: string; states: string[]; thresholds: number[]; currentState: string }>} */
const quantizers = new Map();

/** @type {Map<string, Record<string, number>>} */
const blendOverrides = new Map();

${i}

${c}

/**
 * Compute a CompositeState from the current quantizer state.
 */
function computeState() {
  const discrete = {};
  const blend = {};
  const css = {};
  const glsl = {};
  // WGSL channel — the live state index is emitted below into the fixed
  // state_index struct field (slot 0), mirroring the host emit-wgsl.
  const wgsl = {};
  const aria = {};

  for (const [name, q] of quantizers) {
    const stateStr = q.currentState;
    discrete[name] = stateStr;

    const override = blendOverrides.get(name);
    if (override !== undefined) {
      blend[name] = override;
    } else {
      const weights = {};
      for (const s of q.states) {
        weights[s] = s === stateStr ? 1 : 0;
      }
      blend[name] = weights;
    }

    const keys = projectionKeys(name);
    css[keys.cssKey] = stateStr;

    let stateIndex = 0;
    for (let i = 0; i < q.states.length; i++) {
      if (q.states[i] === stateStr) {
        stateIndex = i;
        break;
      }
    }
    glsl[keys.glslKey] = stateIndex;
    // WGSL: live state index → the single fixed state_index field (slot 0),
    // mirroring the host emit-wgsl so client:worker WGSL shaders see crossings.
    wgsl['state_index'] = stateIndex;
    aria[keys.ariaKey] = stateStr;
  }

  return { discrete, blend, outputs: { css, glsl, wgsl, aria } };
}

// ---------------------------------------------------------------------------
// Canvas rendering
// ---------------------------------------------------------------------------

/**
 * Draw the current CompositeState to the OffscreenCanvas.
 * This is a diagnostic visualization; real applications would
 * implement domain-specific rendering.
 *
 * @param {{ discrete: Record<string, string>; blend: Record<string, Record<string, number>>; outputs: { css: Record<string, number|string>; glsl: Record<string, number>; wgsl: Record<string, number>; aria: Record<string, string> } }} state
 * @param {number} frame
 * @param {number} progress
 */
function drawState(state, frame, progress) {
  if (!ctx || !canvas) return;

  const w = canvas.width;
  const h = canvas.height;

  // Clear
  ctx.clearRect(0, 0, w, h);

  // Background: gradient based on progress
  const gray = Math.round(32 + progress * 32);
  ctx.fillStyle = "rgb(" + gray + "," + gray + "," + gray + ")";
  ctx.fillRect(0, 0, w, h);

  // Draw discrete state labels
  ctx.fillStyle = "#ffffff";
  ctx.font = "14px monospace";
  ctx.textBaseline = "top";

  let y = 16;
  const keys = Object.keys(state.discrete);
  for (let i = 0; i < keys.length; i++) {
    const name = keys[i];
    const value = state.discrete[name];
    ctx.fillText(name + ": " + value, 16, y);
    y += 20;
  }

  // Draw progress bar
  const barY = h - 24;
  const barH = 8;
  ctx.fillStyle = "#333333";
  ctx.fillRect(16, barY, w - 32, barH);
  ctx.fillStyle = "#4488ff";
  ctx.fillRect(16, barY, (w - 32) * progress, barH);

  // Frame counter
  ctx.fillStyle = "#aaaaaa";
  ctx.font = "12px monospace";
  ctx.textBaseline = "bottom";
  ctx.fillText("frame " + frame, 16, barY - 4);
}

// ---------------------------------------------------------------------------
// Render loop
// ---------------------------------------------------------------------------

/**
 * Run the fixed-step render loop.
 *
 * When targetFps is set (via the init message), frames are paced by
 * EMISSION time: after each posted frame the loop waits one full
 * 1000/targetFps budget before drawing the next, so consecutive
 * emissions are never closer than the budget — regardless of how draw
 * cost varies frame to frame. The effective rate may therefore sit
 * slightly below targetFps (budget + draw cost per cycle); targetFps is
 * a "never faster than" production throttle, not an exact-rate
 * scheduler. When targetFps is 0 (omitted at construction), the loop
 * free-runs at maximum speed -- frame timing is then up to the
 * consumer (e.g. an encoding pipeline).
 *
 * @param {{ fps: number; width: number; height: number; durationMs: number }} config
 */
async function runRender(config) {
  if (rendering) return;
  rendering = true;
  stopRequested = false;

  const totalFrames = Math.ceil((config.durationMs / 1000) * config.fps);
  const minFrameIntervalMs = targetFps > 0 ? 1000 / targetFps : 0;

  try {
    for (let i = 0; i < totalFrames; i++) {
      if (stopRequested) break;

      const timestamp = (i * 1000) / config.fps;
      const progress = totalFrames > 1 ? i / (totalFrames - 1) : 1;
      const state = computeState();

      // Draw to canvas
      drawState(state, i, progress);

      /** @type {import('./messages.js').VideoFrameOutput} */
      const output = { frame: i, timestamp, progress, state };

      self.postMessage({ type: "frame", output: output });

      if (minFrameIntervalMs > 0 && i < totalFrames - 1) {
        // Emission-anchored pacing: wait one full budget from THIS frame's
        // emission before drawing the next. The next frame's draw cost
        // lands on top of the wait, so consecutive emissions are never
        // closer than the budget even when draw cost varies (a scheduled-
        // slot deadline would let a cheap frame after an expensive one
        // emit compressed). Inherently burst-proof: there is no schedule
        // to bank debt against. The await also yields the event loop, so
        // stop messages are processed during the wait and honored by the
        // stopRequested check at the top of the next iteration. The final
        // frame skips the wait — there is no next frame to throttle, and
        // render-complete must not lag a dead budget.
        await new Promise(function (r) { setTimeout(r, minFrameIntervalMs); });
      } else if (i % 10 === 9) {
        // Unpaced default: yield periodically to allow stop messages
        // to be processed; frame rate is the consumer's concern.
        await new Promise(function (r) { setTimeout(r, 0); });
      }
    }

    self.postMessage({ type: "render-complete", totalFrames: totalFrames });
  } catch (err) {
    self.postMessage({
      type: "error",
      code: "render-failed",
      message: err instanceof Error ? err.message : String(err),
      hint: "The render loop threw while drawing a frame — check the add-quantizer registrations and the transferred canvas, then re-send start-render.",
    });
  } finally {
    rendering = false;
  }
}

// ---------------------------------------------------------------------------
// Message handler
// ---------------------------------------------------------------------------

self.addEventListener("message", function (e) {
  const msg = e.data;
  if (!msg || typeof msg.type !== "string") return;

  switch (msg.type) {
    case "init": {
      quantizers.clear();
      blendOverrides.clear();
      const fps = msg.config && typeof msg.config.targetFps === "number" ? msg.config.targetFps : 0;
      targetFps = Number.isFinite(fps) && fps > 0 ? fps : 0;
      self.postMessage({ type: "ready" });
      break;
    }

    case "transfer-canvas": {
      canvas = msg.canvas;
      ctx = canvas.getContext("2d");
      break;
    }

    case "add-quantizer": {
      const initialState = msg.states[0] || "";
      quantizers.set(msg.name, {
        id: msg.boundaryId,
        states: Array.from(msg.states),
        thresholds: Array.from(msg.thresholds),
        currentState: initialState,
      });
      break;
    }

    case "remove-quantizer": {
      quantizers.delete(msg.name);
      blendOverrides.delete(msg.name);
      break;
    }

    case "evaluate": {
      const q = quantizers.get(msg.name);
      if (q) {
        q.currentState = evaluateThresholds(q.thresholds, q.states, msg.value);
      }
      break;
    }

    case "set-blend": {
      blendOverrides.set(msg.name, msg.weights);
      break;
    }

    case "start-render": {
      runRender(msg.config);
      break;
    }

    case "stop-render": {
      stopRequested = true;
      break;
    }

    case "dispose": {
      stopRequested = true;
      quantizers.clear();
      blendOverrides.clear();
      canvas = null;
      ctx = null;
      self.close();
      break;
    }
  }
});
`;function Q(e,t,n){n&&n.length>0?e.postMessage(t,n):e.postMessage(t)}function Ge(e){let t=new Blob([We],{type:`application/javascript`}),n=URL.createObjectURL(t),r=new Worker(n,{type:`classic`,name:`czap-renderer`});URL.revokeObjectURL(n);let i=new Set,a=new Set;return r.addEventListener(`message`,e=>{let t=e.data;if(!(!t||typeof t.type!=`string`))switch(t.type){case`frame`:for(let e of i)e(t.output);break;case`render-complete`:for(let e of a)e(t.totalFrames);break;case`error`:s.error({source:`czap/worker.render-worker`,code:`worker-message-error`,message:`Render worker reported an error.`,detail:{code:t.code,message:t.message,hint:t.hint}});break}}),r.addEventListener(`error`,e=>{s.error({source:`czap/worker.render-worker`,code:`worker-unhandled-error`,message:`Render worker raised an unhandled error.`,detail:e.message})}),Q(r,e===void 0?{type:`init`}:{type:`init`,config:e}),{get worker(){return r},transferCanvas(e){Q(r,{type:`transfer-canvas`,canvas:e},[e])},startRender(e){Q(r,{type:`start-render`,config:e})},stopRender(){Q(r,{type:`stop-render`})},onFrame(e){return i.add(e),()=>{i.delete(e)}},onComplete(e){return a.add(e),()=>{a.delete(e)}},dispose(){Q(r,{type:`dispose`}),i.clear(),a.clear(),r.terminate()}}}var Ke={create:Ge};function qe(e,n){let i=Ue.create(e,n),a=null,o=null,s=[];return{get compositor(){return i},get renderer(){return a},attachCanvas(t){a===null&&(a=Ke.create(e)),o={width:t.width,height:t.height};let n=t.transferControlToOffscreen();a.transferCanvas(n)},startRender(e){if(a===null||o===null)throw t(`WorkerHost.canvas`,`cannot start render -- no canvas attached. Call attachCanvas() first.`);let n={durationMs:r(e.durationMs),fps:e.fps??60,width:e.width??o.width,height:e.height??o.height};a.startRender(n)},stopRender(){a!==null&&a.stopRender()},onState(e){let t=i.onState(e);return s.push(t),()=>{let e=s.indexOf(t);e>=0&&s.splice(e,1),t()}},dispose(){for(let e of s)e();s.length=0,i.dispose(),a!==null&&(a.dispose(),a=null)}}}var Je={create:qe};function $(e,t){let n=Object.keys(e),r=Object.keys(t);if(n.length!==r.length)return!1;for(let r of n)if(e[r]!==t[r])return!1;return!0}function Ye(e,t){let n=Object.keys(e),r=Object.keys(t);if(n.length!==r.length)return!1;for(let r of n)if(e[r]!==t[r])return!1;return!0}function Xe(e,t){return Array.isArray(e)||Array.isArray(t)?!Array.isArray(e)||!Array.isArray(t)||e.length!==t.length?!1:e.every((e,n)=>e===t[n]):e===t}function Ze(e,t){let n=Object.keys(e),r=Object.keys(t);if(n.length!==r.length)return!1;for(let r of n){let n=t[r];if(n===void 0||!Xe(e[r],n))return!1}return!0}function Qe(e,t){return e?$(e.discrete,t.discrete)&&$(e.aria,t.aria)&&$(Object.fromEntries(Object.entries(e.css).map(([e,t])=>[e,String(t)])),Object.fromEntries(Object.entries(t.css).map(([e,t])=>[e,String(t)])))&&Ye(e.glsl,t.glsl)&&Ze(e.wgsl,t.wgsl):!1}function $e(){return typeof Worker<`u`&&typeof SharedArrayBuffer<`u`&&globalThis.crossOriginIsolated}function et(e,t){let r=l(t.getAttribute(`data-czap-boundary`));if(!r)return;let i=null,a=null,o=null,c=null,g=null,_=null,v=t.getAttribute(`data-czap-state`)??``,y=null,b=0,x=0,S=!1,C=()=>{i?.(),i=null,o?.(),o=null,c?.(),c=null,g&&_&&typeof _.removeEventListener==`function`&&_.removeEventListener(`message`,g),g=null,_=null,a?.dispose(),a=null},w=()=>f(r.input),T=()=>{let e=(e=!1)=>{if(!r)return;let n=w();if(n===void 0)return;let i=e?p(r,n):p(r,n,v);i!==v&&(v=i,h(t,r,{discrete:{[r.name]:i}},`czap:worker-state`))};e(!0),r&&(i=d(r.input,()=>e(!1)))},E=()=>{if(!r)return;let e=r,s=Je.create();a=s;let l=(t,r,i=!1)=>{let a=[{name:e.name,state:n(t),generation:r}];if(S=!0,b=r,x=r,i){s.compositor.bootstrapResolvedState(a);return}s.compositor.applyResolvedState(a)},f=(n,r)=>{let i={discrete:{[e.name]:n}};h(t,e,i,`czap:worker-state`),v=n,y=u(i),x=r};s.compositor.addQuantizer(e.name,{id:e.boundary.id,states:e.boundary.states.map(e=>n(e)),thresholds:e.boundary.thresholds});let m=e=>{e.data?.type===`ready`&&t.dispatchEvent(new CustomEvent(`czap:worker-ready`,{bubbles:!0}))};g=m,_=s.compositor.worker,s.compositor.worker.addEventListener(`message`,m),c=s.compositor.onResolvedStateAck(t=>{if(a!==s||r!==e)return;let n=t.states.find(t=>t.name===e.name)?.state;S&&t.additionalOutputsChanged===!1&&t.generation===b&&n!==void 0&&n===v&&(S=!1)}),o=s.onState(n=>{let r=n.discrete?.[e.name];r&&(v=r);let i=u(n),a=n.resolvedStateGenerations?.[e.name];if(S&&a!==void 0&&a===b&&r===y?.discrete[e.name]&&Qe(y,i)){S=!1;return}h(t,e,n,`czap:worker-state`),y=i,a!==void 0&&(x=a,S=!1)});let C=()=>{if(a!==s||r!==e)return;let t=w();if(t===void 0)return;let n=p(e,t,v||void 0);if(n===v)return;let i=x+1;f(n,i),l(n,i)},T=w();if(T!==void 0){let t=p(e,T,v||void 0);f(t,1),l(t,1,!0)}i=d(e.input,C)},D=()=>{if(r&&m(r.input,{source:`czap/astro.worker`,what:`boundary signal`}),!$e()){s.warnOnce({source:`czap/astro.worker`,code:`worker-runtime-unavailable`,message:`Worker runtime unavailable (crossOriginIsolated=${String(globalThis.crossOriginIsolated)}, SharedArrayBuffer=${typeof SharedArrayBuffer<`u`}). Fix: czap({ workers: { enabled: true } }) — COOP/COEP response headers are emitted automatically.`}),T();return}try{E();return}catch(e){s.warn({source:`czap/astro.worker`,code:`worker-host-fallback`,message:`WorkerHost could not initialize, falling back to main-thread evaluation.`,detail:e instanceof Error?e.message:String(e)})}T()};t.addEventListener(`czap:reinit`,()=>{C(),r=l(t.getAttribute(`data-czap-boundary`)),v=t.getAttribute(`data-czap-state`)??``,y=null,b=0,x=0,S=!1,c=null,D()}),t.addEventListener(`czap:teardown`,()=>{C()}),D(),e()}var tt=(e,t,n)=>{_(`worker`,e,t,n,(e,t,n)=>{et(e,n)})};export{tt as default};