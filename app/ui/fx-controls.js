// What the sections of the panel under the waveform share: the ruler every value is set with,
// and the axes of their graphs.

/** @typedef {Record<string, number | boolean>} Params */
/** @typedef {HTMLLabelElement & { sync: () => void }} Slider */

// The frequency axis of the panel's graphs, and the EQ's gain range in dB either way.
export const FMIN = 20, FMAX = 20000, DBR = 24;
/** Where a frequency sits on a log axis W wide. @param {number} hz @param {number} W */
export const freqX = (hz, W) => Math.log(hz / FMIN) / Math.log(FMAX / FMIN) * W;
/** @param {number} x @param {number} W */
export const xFreq = (x, W) => FMIN * Math.pow(FMAX / FMIN, x / W);
/** Where a gain in dB sits on the EQ graph, H high. @param {number} g @param {number} H */
export const gainY = (g, H) => H / 2 - g / DBR * (H / 2 - 14);
/** @param {number} y @param {number} H */
export const yGain = (y, H) => (H / 2 - y) / (H / 2 - 14) * DBR;
/** @param {number} g */
export const clampGain = (g) => Math.max(-DBR, Math.min(DBR, g));

/**
 * @param {string} label @param {string} key @param {number} min @param {number} max @param {number} step
 * @param {string} unit @param {Params} obj @param {() => void} onInput @returns {Slider}
 */
export function slider(label, key, min, max, step, unit, obj, onInput) {
  const wrap = /** @type {Slider} */ (document.createElement('label'));
  wrap.className = 'sl';
  wrap.innerHTML = `<span>${label}</span><input type="range" min="${min}" max="${max}" step="${step}"><output></output>`;
  const inp = /** @type {HTMLInputElement} */ (wrap.querySelector('input')), out = /** @type {HTMLOutputElement} */ (wrap.querySelector('output'));
  const show = () => { out.textContent = `${Number(obj[key]).toFixed(step < 1 ? (step < 0.1 ? 2 : 1) : 0)}${unit}`; };
  inp.value = String(obj[key]); show();
  inp.oninput = () => { obj[key] = Number(inp.value); show(); onInput(); };
  wrap.sync = () => { inp.value = String(obj[key]); show(); };
  // Double-click the readout to type an exact value.
  out.dataset.tip = 'Double-click to type a value';
  out.ondblclick = (e) => {
    e.preventDefault();
    const box = document.createElement('input');
    box.type = 'number'; box.min = String(min); box.max = String(max); box.step = 'any'; box.value = String(obj[key]); box.className = 'sl-type';
    box.setAttribute('aria-label', `${label}, between ${min} and ${max}`);
    let done = false;
    const finish = (/** @type {boolean} */ keep) => {
      if (done) return; done = true;
      const v = Number(box.value);
      box.replaceWith(out);
      if (keep && box.value !== '' && isFinite(v)) { obj[key] = Math.min(max, Math.max(min, v)); wrap.sync(); onInput(); }
    };
    box.onkeydown = (ev) => { ev.stopPropagation(); if (ev.key === 'Enter') finish(true); else if (ev.key === 'Escape') finish(false); };
    box.onblur = () => finish(true);
    out.replaceWith(box); box.focus(); box.select();
  };
  return wrap;
}

// A row of one-click values under the rulers.
/** @param {[string, () => void, string?][]} items */
export function presetRow(items) {
  const row = document.createElement('div'); row.className = 'fx-presets';
  for (const [label, fn, tip] of items) {
    const b = document.createElement('button'); b.type = 'button'; b.textContent = label; b.onclick = fn;
    if (tip) b.dataset.tip = tip;
    row.append(b);
  }
  return row;
}
