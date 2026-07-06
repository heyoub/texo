function e(e){return e.replace(/-/g,`_`).replace(/([a-z0-9])([A-Z])/g,`$1_$2`).toLowerCase()}function t(t){return`u_${e(t)}`}var n=`/**
 * Per-quantizer output keys, matching @czap/core projectionKeys / glslIdent / wgslIdent.
 * @param {string} name
 * @returns {{ cssKey: string, glslKey: string, wgslKey: string, ariaKey: string }}
 */
function projectionKeys(name) {
  const snake = name.replace(/-/g, "_").replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();
  return { cssKey: "--czap-" + name, glslKey: "u_" + snake, wgslKey: snake, ariaKey: "data-czap-" + name };
}`;export{t as n,n as t};