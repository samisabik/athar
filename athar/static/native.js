// The library and app commands of the native side, one function each. Errors arrive as Error
// objects carrying the native message.

import { invoke } from './engine.js';

/** Where and how a new file is written. @typedef {{ dest_dir: string, name: string, format?: string, source?: string, project?: number, sp404?: boolean }} SaveMeta */

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
  projectOrder: (/** @type {number} */ id) => call('project_order', { id: +id }),
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
  setArrangement: req('set_arrangement'),
  // Saves render natively and write a new file. `meta` is { dest_dir, name, format, source?,
  // project?, sp404? }; each resolves with { path } (and the mix with how far over it was).
  saveTake: (/** @type {string} */ key, /** @type {SaveMeta} */ meta) => call('rec_save', { key, meta }),
  saveBatch: (/** @type {string} */ path, /** @type {object} */ p, /** @type {SaveMeta} */ meta) => call('batch_save', { path, p, meta }),
  saveMix: (/** @type {object} */ p, /** @type {SaveMeta} */ meta) => call('arr_save', { p, meta }),
};

// The window and its dialogs.
export const app = {
  pickFolder: (start = '') => call('pick_folder', { start: start || null }),
  pickFile: () => call('pick_file'),
  setDirty: (/** @type {boolean} */ dirty) => call('set_dirty', { dirty: !!dirty }),
  quitNow: () => call('quit_now'),
};
