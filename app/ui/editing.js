// The editor as the rest of the window uses it: the one Editor with its effects panel, opening a
// sound in it, and where its saves go.

import { $, toast, notice, fail, modal, confirmModal, choose, pickFolder } from './ui.js';
import { Editor } from './editor.js';
import { FxPanel } from './fx.js';
import { invoke } from './engine.js';
import { lib } from './native.js';
import { state } from './store.js';
import { SETTINGS, onSetting } from './settings.js';
import { fmtTime } from './time.js';
import { baseOf, dirOf } from './names.js';
import { projectOf, projectOptions } from './places.js';
import { registerPage, showView } from './views.js';
import { registerPlayer, stopOthers } from './players.js';
import { catContext } from './cat.js';
import { loadProjects, refreshSideSoon } from './sidebar.js';

/** @typedef {import('./search.js').Project} Project */
/** @typedef {import('./editor.js').SaveTarget} SaveTarget */

// The editor always saves a new file. This asks for its name and where it goes: a project,
// next to the sound it was made from, or another folder.
/** @type {string | null} */
let lastSaveTarget = null;
/** @param {string} [title] @returns {Promise<SaveTarget | null>} */
async function whereToSave(title) {
  try { state.projects = await lib.projects(); } catch { }
  const home = projectOf(editor.path), nameIn = editor.field('name'), dir = editor.dir;
  const beside = dir && dir !== home?.folder ? dir : null;
  /** @type {[string, string][]} */
  const options = [
    ...projectOptions(state.projects),
    ...(beside ? /** @type {[string, string][]} */ ([['beside', `Library: next to the original, in ${baseOf(beside)}`]]) : []),
    ['folder', 'Another folder…'],
  ];
  const want = home ? `p:${home.id}` : lastSaveTarget;
  const value = /** @type {string} */ (options.some(o => o[0] === want) ? want : beside ? 'beside' : options[0][0]);
  const v = await modal(title || 'Save as a new file', [
    { label: 'Name', value: nameIn.value },
    { label: editor.path ? 'Save into. The original file stays as it is' : 'Save into', options, value },
  ]);
  if (!v || !v[0]) return null;
  const [name, place] = v;
  nameIn.value = name;
  if (place === 'folder') { const picked = await pickFolder('Save into'); return picked ? { dir: picked, where: baseOf(picked) } : null; }
  lastSaveTarget = place;
  if (place === 'beside' && beside) return { dir: beside, where: baseOf(beside) };
  const p = /** @type {Project} */ (state.projects.find(x => `p:${x.id}` === place));
  return { dir: p.folder || dir || '.', project: p.id, where: `the project ${p.name}` };
}

/** @type {Editor} */
export const editor = new Editor($('#editor'), {
  toast, notice, pickFolder,
  ask: async (title, value) => (await modal(title, [{ label: 'Name', value }]))?.[0],
  // p says how to render the editor's sound; the native side renders and writes it.
  onSave: async (p, dir, name, opts = {}) => {
    const meta = { dest_dir: dir, name, sp404: !!opts.sp404, format: opts.sp404 ? 'wav' : opts.format || 'wav', source: opts.source || '', project: opts.project || 0 };
    try {
      const { path } = await invoke('ed_save', { p, meta });
      if (opts.project) { loadProjects().catch(() => { }); refreshSideSoon(); }
      return path;
    }
    catch (e) { fail(e); return null; }
  },
  whereToSave,
  place: () => projectOf(editor.path)?.name || 'Library',
});
editor.settings = SETTINGS;
editor.fx = new FxPanel($('#fx'), editor, toast, notice);
editor.fx.choose = choose;
editor.fx.onSection = () => catContext();
document.querySelectorAll('[data-fx-open]').forEach(b => b.onclick = () => editor.fx.open(/** @type {string} */ (b.dataset.fxOpen)));

// Before a sound is opened, only the waveform's place (where it says how to open one) takes input.
for (const type of ['mousedown', 'click', 'dblclick', 'change', 'keydown']) {
  $('#editor').addEventListener(type, (/** @type {Event} */ e) => {
    if ($('#editor').classList.contains('blank') && !/** @type {Element} */ (e.target).closest('#wave')) { e.preventDefault(); e.stopPropagation(); }
  }, true);
}

// Playing and auditioning in the editor silence every other player, and the other way round.
editor.onAudition = () => stopOthers('editor');
{
  const play = editor.play.bind(editor);
  editor.play = (/** @type {Parameters<typeof play>} */ ...args) => { stopOthers('editor'); return play(...args); };
}
function stopEditor() { editor.stopAudition(); if (editor.playing) editor.stopPlay(); }
registerPlayer('editor', stopEditor);
registerPage('editor', { show: () => requestAnimationFrame(() => editor.draw()), leave: stopEditor });

// The waveform and the effect panels are drawn in the theme's colours.
onSetting(['theme', 'accent'], () => { editor.layerKey = null; editor.draw(); editor.fx.redraw(); });
// The editor names the project its sound is in, so it follows the projects being renamed or removed.
window.addEventListener('athar:projects', () => { if (editor.loaded) editor.refreshInfo(); });

// The editor holds a whole sound in memory as 32-bit samples, with further copies for playback and undo.
const EDITOR_ASK_BYTES = 1.2e9;
/** Names the latest request to open a sound; an older one that is still waiting gives way. */
let openSeq = 0;
/**
 * Opens a sound in the editor and shows the editor, after asking about unsaved edits and about a
 * very long sound. `dir` is the folder its saves are offered next to, or what finds it.
 * @param {{ source: { path: string } | { key: string }, name: string, dir: string | (() => Promise<string>), tempo?: number | null,
 *   duration?: number | null, sr?: number | null, channels?: number | null }} sound
 */
export async function openInEditor({ source, name, dir, tempo = null, duration, sr, channels }) {
  const bytes = (duration || 0) * (sr || 48000) * (channels || 2) * 4;
  if (bytes > EDITOR_ASK_BYTES && !await confirmModal('Open a long recording?', `${name} is ${fmtTime(duration)} long and needs about ${(bytes * 3 / 1e9).toFixed(1)} GB of memory in the editor.`, 'Open')) return;
  if (editor.dirty && !await confirmModal('Discard unsaved edits?', `${editor.name} has edits that are not saved. Opening ${name} discards them.`, 'Discard')) return;
  const seq = ++openSeq;
  try {
    const folder = typeof dir === 'function' ? await dir() : dir;
    if (seq !== openSeq) return;
    editor.fx.clearPreview();
    showView('editor');
    await editor.load({ source, name, dir: folder, tempo });
  } catch (e) { if (seq === openSeq) fail(new Error(`Could not open ${name}: ${e instanceof Error ? e.message : e}`)); }
}
/** Opens a sound of the Library in the editor. @param {{ path: string, name: string, tempo?: number | null, duration?: number | null, sr?: number | null, channels?: number | null }} it */
export function openPath(it) {
  return openInEditor({ source: { path: it.path }, name: it.name, dir: dirOf(it.path), tempo: it.tempo, duration: it.duration, sr: it.sr, channels: it.channels });
}
