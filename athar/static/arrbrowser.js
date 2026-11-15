import { fmtTime } from './audio.js';

const LIMIT = 200;

/** The Arranger's own browser. It lists the sounds of one project. A sound is heard with its play
    button and goes into the arrangement by dragging it onto the lanes. A double-click, or Enter on
    its row, adds it at the start. */
export class ArrBrowser {
  constructor(h, onAdd) {
    this.h = h; this.onAdd = onAdd;
    this.project = ''; this.items = []; this.dragging = null; this.seq = 0;
    const { el } = h;
    this.proj = el('select', { 'aria-label': 'Project' });
    this.proj.onchange = () => { this.project = this.proj.value; this.load(); };
    this.count = el('div', { class: 'arb-count muted' });
    this.list = el('div', { class: 'arb-list' });
    this.root = el('aside', { class: 'arb', 'aria-label': 'Sounds to arrange' }, this.proj, this.count, this.list);
  }

  // Projects can change while another page is open, so the list is read again each visit.
  async show() {
    const { api, el } = this.h;
    try {
      const projects = await api('/api/projects');
      this.proj.replaceChildren(...(projects.length ? projects.map(p => el('option', { value: String(p.id) }, p.name)) : [el('option', { value: '' }, 'No projects yet')]));
      if (!projects.some(p => String(p.id) === this.project)) this.project = projects[0] ? String(projects[0].id) : '';
      this.proj.value = this.project;
    } catch (e) { this.h.fail(e); }
    return this.load();
  }

  async load() {
    const seq = ++this.seq;
    if (!this.project) { this.items = []; return this.render('Create a project on the Projects page to browse it here.'); }
    try {
      const items = await this.h.api('/api/search', { json: { project: Number(this.project), sort: 'name', limit: LIMIT } });
      if (seq !== this.seq) return;
      this.items = items;
      this.render(items.length ? '' : 'This project has no sounds yet.');
    } catch (e) { if (seq === this.seq) this.h.fail(e); }
  }

  render(empty = '') {
    const { el, ib, miniWave, focusable } = this.h, n = this.items.length;
    this.count.textContent = empty || `${n === LIMIT ? `First ${LIMIT}` : n} sound${n === 1 ? '' : 's'}`;
    this.list.replaceChildren(...this.items.map((it) => {
      const add = () => this.onAdd(it, null);
      return focusable(el('div', { class: 'arb-row', draggable: 'true', ondblclick: add,
        ondragstart: (e) => { this.dragging = it; e.dataTransfer?.setData('text/plain', it.name); if (e.dataTransfer) e.dataTransfer.effectAllowed = 'copy'; },
        ondragend: () => { this.dragging = null; this.onDragEnd?.(); } },
      Object.assign(ib(this.h.isPlaying(it) ? 'stop' : 'play_arrow', this.h.isPlaying(it) ? 'Stop' : 'Play', (e) => { e.stopPropagation(); this.h.preview(it); }, 'pplay'), { playPath: it.path }),
      miniWave(it),
      el('span', { class: 'qn' }, it.name),
      el('span', { class: 'qd' }, fmtTime(it.duration))), add);
    }));
  }
}
