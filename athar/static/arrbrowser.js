import { fmtTime } from './audio.js';

const LIMIT = 200, TYPE_WAIT_MS = 350;

/** The Arranger's own browser. It lists the Library, by search and folder, or the sounds of one
    project. A sound is heard with its play button and goes into the arrangement by dragging it
    onto the lanes. A double-click, or Enter on its row, adds it at the start. */
export class ArrBrowser {
  constructor(h, onAdd) {
    this.h = h; this.onAdd = onAdd;
    this.tab = 'library'; this.query = ''; this.where = ''; this.project = ''; this.items = []; this.dragging = null; this.seq = 0;
    const { el } = h;
    const tab = (key, label) => el('button', { type: 'button', 'aria-pressed': 'false', onclick: () => { this.tab = key; this.dress(); this.load(); } }, label);
    this.tabs = { library: tab('library', 'Library'), projects: tab('projects', 'Projects') };
    this.q = el('input', { placeholder: 'Describe a sound', spellcheck: 'false', autocomplete: 'off', 'aria-label': 'Search the Library' });
    this.q.oninput = () => { clearTimeout(this.typing); this.typing = setTimeout(() => { this.query = this.q.value.trim(); this.load(); }, TYPE_WAIT_MS); };
    this.q.onkeydown = (e) => { if (e.key === 'Enter') { clearTimeout(this.typing); this.query = this.q.value.trim(); this.load(); } };
    this.folder = el('select', { 'aria-label': 'Folder' });
    this.folder.onchange = () => { this.where = this.folder.value; this.load(); };
    this.proj = el('select', { 'aria-label': 'Project' });
    this.proj.onchange = () => { this.project = this.proj.value; this.load(); };
    this.count = el('div', { class: 'arb-count muted' });
    this.list = el('div', { class: 'arb-list' });
    this.root = el('aside', { class: 'arb', 'aria-label': 'Sounds to arrange' },
      el('div', { class: 'arb-tabs', role: 'group', 'aria-label': 'Where to browse' }, this.tabs.library, this.tabs.projects),
      this.q, this.folder, this.proj, this.count, this.list);
    this.dress();
  }

  dress() {
    const lib = this.tab === 'library';
    for (const [key, b] of Object.entries(this.tabs)) { b.classList.toggle('on', key === this.tab); b.setAttribute('aria-pressed', String(key === this.tab)); }
    this.q.hidden = this.folder.hidden = !lib; this.proj.hidden = lib;
  }

  // The folders and projects can change while another page is open, so the two lists are read again each visit.
  async show() {
    const { api, el } = this.h;
    try {
      const [folders, projects] = await Promise.all([api('/api/folders'), api('/api/projects')]);
      const options = [el('option', { value: '' }, 'All folders')];
      for (const f of folders.filter(f => !f.offline)) {
        options.push(el('option', { value: JSON.stringify([f.path, null]) }, f.path.split('/').pop()));
        for (const s of await api('/api/subfolders', { json: { path: f.path } }).catch(() => [])) {
          if (s.files) options.push(el('option', { value: JSON.stringify([f.path, s.path]) }, `  ${s.name}`));
        }
      }
      this.folder.replaceChildren(...options);
      this.folder.value = options.some(o => o.value === this.where) ? this.where : (this.where = '');
      this.proj.replaceChildren(...(projects.length ? projects.map(p => el('option', { value: String(p.id) }, p.name)) : [el('option', { value: '' }, 'No projects yet')]));
      if (!projects.some(p => String(p.id) === this.project)) this.project = projects[0] ? String(projects[0].id) : '';
      this.proj.value = this.project;
    } catch (e) { this.h.fail(e); }
    return this.load();
  }

  async load() {
    const seq = ++this.seq, lib = this.tab === 'library';
    const [folder, under] = lib && this.where ? JSON.parse(this.where) : [null, null];
    if (!lib && !this.project) { this.items = []; return this.render('Create a project on the Projects page to browse it here.'); }
    try {
      const items = await this.h.api('/api/search', { json: lib
        ? { query: this.query || null, folder, under, sort: this.query ? 'score' : 'name', limit: LIMIT }
        : { project: Number(this.project), sort: 'name', limit: LIMIT } });
      if (seq !== this.seq) return;
      this.items = items;
      this.render(items.length ? '' : lib ? 'No sounds match.' : 'This project has no sounds yet.');
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
