// Range inputs draw their filled part from a --f custom property; keep it in step with the value.
function sync(r) {
  const min = Number(r.min || 0), max = Number(r.max || 100);
  r.style.setProperty('--f', String((Number(r.value) - min) / ((max - min) || 1)));
}
const desc = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value');
Object.defineProperty(HTMLInputElement.prototype, 'value', {
  get() { return desc.get.call(this); },
  set(v) { desc.set.call(this, v); if (this.type === 'range') sync(this); },
  configurable: true,
});
document.addEventListener('input', (e) => { if (e.target.type === 'range') sync(e.target); }, true);
// Ranges are only created in the settings form and the effects panel.
const observer = new MutationObserver((muts) => {
  for (const m of muts) for (const n of m.addedNodes) {
    if (n.nodeType !== 1) continue;
    if (n.matches('input[type=range]')) sync(n);
    n.querySelectorAll('input[type=range]').forEach(sync);
  }
});
for (const id of ['settingsForm', 'fx']) {
  const root = document.getElementById(id);
  if (root) observer.observe(root, { childList: true, subtree: true });
}
document.querySelectorAll('input[type=range]').forEach(sync);
