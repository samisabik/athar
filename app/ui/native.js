// The library, app and editor commands of the native side, one function each. Errors arrive as
// Error objects carrying the native message.

import { invoke } from './engine.js';

/** Where and how a new file is written. `name` is a file name with its extension; the native save keeps its stem and writes `format`. @typedef {{ dest_dir: string, name: string, format?: string, source?: string, project?: number, sp404?: boolean }} SaveMeta */

/** @param {string} cmd @param {unknown} [args] @param {{ headers?: Record<string, string> }} [opts] @returns {Promise<any>} */
const call = (cmd, args, opts) => invoke(cmd, args, opts).catch((e) => { throw new Error(typeof e === 'string' ? e : e?.message || String(e)); });
/** A command that takes one request object. @param {string} cmd */
const req = (cmd) => (/** @type {any} */ r) => call(cmd, { req: r });
/** A command about one project. @param {string} cmd */
const inProject = (cmd) => (/** @type {number | string} */ id, /** @type {any} */ r) => call(cmd, { id: +id, req: r });

export const lib = {
  status: () => call('status'),
  settings: () => call('get_settings'),
  setSettings: req('set_settings'),
  folders: () => call('folders'),
  addFolder: req('add_folder'),
  removeFolder: req('remove_folder'),
  subfolders: req('subfolders'),
  rescan: () => call('rescan'),
  categories: () => call('categories'),
  search: req('search'),
  soundMap: () => call('sound_map'),
  tags: () => call('tags'),
  catalog: req('catalog'),
  trash: req('trash'),
  rename: req('rename'),
  reveal: req('reveal'),
  revealFolder: req('reveal_folder'),
  revealData: () => call('reveal_data'),
  sp404Card: () => call('sp404_card'),
  projects: () => call('projects'),
  createProject: req('create_project'),
  renameProject: inProject('rename_project'),
  deleteProject: (/** @type {number} */ id) => call('delete_project', { id: +id }),
  projectItem: inProject('project_item'),
  projectFolder: inProject('project_folder'),
  setProjectOrder: inProject('set_project_order'),
  projectPads: (/** @type {number} */ id) => call('project_pads', { id: +id }),
  setProjectPads: inProject('set_project_pads'),
  exportProject: inProject('export_project'),
  backup: req('backup'),
  restore: req('restore'),
  discoverSources: () => call('discover_sources'),
  discoverSearch: req('discover_search'),
  discoverWaveform: req('discover_waveform'),
  discoverFiles: req('discover_files'),
  discoverSave: req('discover_save'),
  arrangement: () => call('arrangement'),
  // Saves render natively and write a new file. `meta` is { dest_dir, name, format, source?,
  // project?, sp404? }; each resolves with { path } (and the mix with how far over it was).
  saveTake: (/** @type {string} */ key, /** @type {SaveMeta} */ meta) => call('rec_save', { key, meta }),
  saveBatch: (/** @type {string} */ path, /** @type {object} */ p, /** @type {SaveMeta} */ meta) => call('batch_save', { path, p, meta }),
  saveMix: (/** @type {object} */ p, /** @type {SaveMeta} */ meta) => call('arr_save', { p, meta }),
};

// The window and its dialogs.
export const app = {
  pickFolder: (title = '', start = '') => call('pick_folder', { title: title || null, start: start || null }),
  pickFile: () => call('pick_file'),
  setDirty: (/** @type {boolean} */ dirty) => call('set_dirty', { dirty: !!dirty }),
  quitNow: () => call('quit_now'),
};

/** What the page knows of the editor's view: its selection and cursor, sent along with every change. @typedef {{ sel: number[] | null, cursor: number }} DocUi */
/** An edit, a region change or an analysis, by `kind` or `op`; app/src/doc.rs reads the fields. @typedef {Record<string, unknown>} DocRequest */
/** What to play: [a, b) of the sound `src` from `start`, in samples. @typedef {{ src: string, a: number, b: number, start: number, loop: boolean, eq?: boolean, fade_in?: number, fade_out?: number }} DocPlay */

// The editor's document: the sound being edited, held natively with its history, regions and
// clipboard. `src` names a sound to show or play: 'doc' for the document itself, or the key of
// a preview made with view().
export const doc = {
  /** @param {{ path: string } | { key: string }} source A library file, or a sound the engine holds. */
  open: (source) => call('ed_open', source),
  /** Without `ui` the edit leaves the selection and cursor alone, as Copy does. @param {DocRequest} p @param {DocUi} [ui] */
  edit: (p, ui) => call('ed_edit', { p, ui }),
  /** @param {DocRequest} p @param {DocUi} ui */
  regions: (p, ui) => call('ed_regions', { p, ui }),
  /** @param {boolean} redo @param {DocUi} ui */
  undo: (redo, ui) => call('ed_undo', { redo, ui }),
  /** The sound at `version` is now the file `path`. @param {string} path @param {number} version @returns {Promise<{ dirty: boolean, path: string | null }>} */
  saved: (path, version) => call('ed_saved', { path, version }),
  /**
   * Works out the sound as the edit `p` would leave it and keeps it under `key`, to show and to
   * play. With `headS`, only that many seconds from the start of the edit's range.
   * @param {string} key @param {DocRequest} p @param {number | null} [headS]
   * @returns {Promise<{ length: number, n?: number, info?: string }>}
   */
  view: (key, p, headS = null) => call('ed_view', { key, p, headS }),
  /** The waveform's points for `width` pixels from sample `start`, as 32-bit floats. @param {string} src @param {number} start @param {number} spp @param {number} width @returns {Promise<ArrayBuffer>} */
  peaks: (src, start, spp, width) => call('ed_peaks', { src, start, spp, width }),
  /** One spectrogram tile, `pw` by `rows` pixels of RGBA. @param {string} src @param {number} start @param {number} spp @param {number} pw @param {number} rows @returns {Promise<ArrayBuffer>} */
  spec: (src, start, spp, pw, rows) => call('ed_spec', { src, start, spp, pw, rows }),
  /** @param {DocRequest} p */
  analyse: (p) => call('ed_analyse', { p }),
  /** @param {DocPlay} p @returns {Promise<{ id: number, at: number }>} The voice, and the engine time it starts at. */
  play: (p) => call('ed_play', { p }),
  /** Stops a voice, over `fade` seconds when given. A voice that has already ended is not an error. @param {number} id @param {number} [fade] */
  stop: (id, fade) => call('ed_stop', { id, fade }).catch(() => { }),
  /** The bands of the live EQ. @param {object[]} bands */
  eq: (bands) => call('ed_eq', { bands }),
  /** Learns the noise print from a span of the sound; null forgets it. @param {[number, number] | null} range */
  noise: (range) => call('ed_noise', { range }),
  /** Writes the sound wherever the user picks; resolves with the path, or null when cancelled. @param {object} p @param {string} name @returns {Promise<string | null>} */
  download: (p, name) => call('ed_download', { p, name }),
};

// What the effect panels draw, computed by the same code that processes the sound.
export const effects = {
  /** Each band's gain in dB at each frequency. @param {object[]} bands @param {number[]} freqs @returns {Promise<number[][]>} */
  eqResponse: (bands, freqs) => call('eq_response', { bands, freqs }),
  /** Output level for `n` input levels between `from` and `to` dB. @param {string} tab @param {object} p @param {number} from @param {number} to @param {number} n @returns {Promise<number[]>} */
  curve: (tab, p, from, to, n) => call('fx_curve', { tab, p, from, to, n }),
  /** Whether the engine sends the spectrum of what plays. @param {boolean} on */
  watchSpectrum: (on) => call('audio_watch_spectrum', { on }),
};
