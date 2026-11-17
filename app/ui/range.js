// Range inputs draw their filled part from a --f custom property; keep it in step with the value.
/** @param {Element} el */
function sync(el) {
  const r = /** @type {HTMLInputElement} */ (el);
  const min = Number(r.min || 0), max = Number(r.max || 100);
  r.style.setProperty('--f', String((Number(r.value) - min) / ((max - min) || 1)));
}
const desc = /** @type {PropertyDescriptor & { get(): string, set(v: string): void }} */ (Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value'));
Object.defineProperty(HTMLInputElement.prototype, 'value', {
  /** @this {HTMLInputElement} */
  get() { return desc.get.call(this); },
  /** @this {HTMLInputElement} @param {string} v */
  set(v) { desc.set.call(this, v); if (this.type === 'range') sync(this); },
  configurable: true,
});
document.addEventListener('input', (e) => { const t = /** @type {HTMLInputElement} */ (e.target); if (t.type === 'range') sync(t); }, true);
// Ranges are only created in the settings form and the effects panel.
const observer = new MutationObserver((muts) => {
  for (const m of muts) for (const node of m.addedNodes) {
    if (node.nodeType !== 1) continue;
    const n = /** @type {Element} */ (node);
    if (n.matches('input[type=range]')) sync(n);
    n.querySelectorAll('input[type=range]').forEach(sync);
  }
});
for (const id of ['settingsForm', 'fx']) {
  const root = document.getElementById(id);
  if (root) observer.observe(root, { childList: true, subtree: true });
}
document.querySelectorAll('input[type=range]').forEach(sync);
