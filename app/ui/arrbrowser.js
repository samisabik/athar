import { el, fail, focusable } from './ui.js';
import { lib } from './native.js';
import { fmtTime } from './time.js';
import { plural } from './names.js';
import { previewButton } from './preview.js';
import { miniWave } from './search.js';

const LIMIT = 200;

/** @typedef {import('./search.js').Item} Item */
/** @typedef {import('./search.js').Project} Project */

/** The Arranger's own browser. It lists the sounds of one project. A sound is heard with its play
    button and goes into the arrangement by dragging it onto the lanes. A double-click, or Enter on
    its row, adds it at the start. */
export class ArrBrowser {
  /** @param {(it: Item) => void} onAdd */
  constructor(onAdd) {
    /** Told when a sound dragged from here is let go. @type {(() => void) | null} */
    this.onDragEnd = null;
    this.onAdd = onAdd;
    this.project = ''; this.seq = 0;
    /** @type {Item[]} */ this.items = [];
    /** The sound being dragged out to the lanes. @type {Item | null} */ this.dragging = null;
    /** @type {HTMLSelectElement} */
    this.proj = el('select', { 'aria-label': 'Project' });
    this.proj.onchange = () => { this.project = this.proj.value; this.load(); };
    this.count = el('div', { class: 'arb-count muted' });
    this.list = el('div', { class: 'arb-list' });
    this.root = el('aside', { class: 'arb', 'aria-label': 'Sounds to arrange' }, this.proj, this.count, this.list);
  }

  // Projects can change while another page is open, so the list is read again each visit.
  async show() {
    try {
      /** @type {Project[]} */
      const projects = await lib.projects();
      this.proj.replaceChildren(...(projects.length ? projects.map(p => el('option', { value: String(p.id) }, p.name)) : [el('option', { value: '' }, 'No projects yet')]));
      if (!projects.some(p => String(p.id) === this.project)) this.project = projects[0] ? String(projects[0].id) : '';
      this.proj.value = this.project;
    } catch (e) { fail(e); }
    return this.load();
  }

  async load() {
    const seq = ++this.seq;
    if (!this.project) { this.items = []; return this.render('Create a project on the Projects page to browse it here.'); }
    try {
      const items = await lib.search({ project: Number(this.project), sort: 'name', limit: LIMIT });
      if (seq !== this.seq) return;
      this.items = items;
      this.render(items.length ? '' : 'This project has no sounds yet.');
    } catch (e) { if (seq === this.seq) fail(e); }
  }

  render(empty = '') {
    const n = this.items.length;
    this.count.textContent = empty || (n === LIMIT ? `First ${LIMIT} sounds` : plural(n, 'sound'));
    this.list.replaceChildren(...this.items.map((it) => {
      const add = () => this.onAdd(it);
      return focusable(el('div', { class: 'arb-row', draggable: 'true', ondblclick: add,
        ondragstart: (/** @type {DragEvent} */ e) => { this.dragging = it; e.dataTransfer?.setData('text/plain', it.name); if (e.dataTransfer) e.dataTransfer.effectAllowed = 'copy'; },
        ondragend: () => { this.dragging = null; this.onDragEnd?.(); } },
      previewButton(it),
      miniWave(it),
      el('span', { class: 'qn' }, it.name),
      el('span', { class: 'qd' }, fmtTime(it.duration))), add);
    }));
  }
}
