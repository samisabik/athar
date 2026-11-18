// One page shows at a time, chosen from the strip on the left.

import { $ } from './ui.js';
import { state } from './store.js';

/** What a page does when it comes into view, and when another page takes its place. @typedef {{ show?: () => void, leave?: () => void }} Page */
/** @type {Map<string, Page>} */
const pages = new Map();

/** @param {string} name The page's data-view. @param {Page} page */
export function registerPage(name, page) { pages.set(name, page); }

/** Shows the page `name`. Every other page is told it is not in view, then 'athar:view' goes out on the window. @param {string} name */
export function showView(name) {
  state.currentView = name;
  document.querySelectorAll('main > .view').forEach(v => v.hidden = v.dataset.view !== name);
  document.querySelectorAll('.ab[data-view]').forEach(b => {
    // The Library folders page is reached from the Library and has no button of its own.
    const on = b.dataset.view === (name === 'folders' ? 'sounds' : name);
    b.classList.toggle('on', on);
    if (on) b.setAttribute('aria-current', 'page'); else b.removeAttribute('aria-current');
  });
  for (const [other, page] of pages) if (other !== name) page.leave?.();
  pages.get(name)?.show?.();
  window.dispatchEvent(new Event('athar:view'));
}

document.querySelectorAll('.ab[data-view]').forEach(b => b.onclick = () => showView(/** @type {string} */ (b.dataset.view)));
document.querySelectorAll('[data-folders]').forEach(b => b.onclick = () => { showView('folders'); if (b.dataset.folders === 'add') $('#addFolder').click(); });
$('#foldersBack').onclick = () => showView('sounds');
