// Where a sound lives, and where a new one can be saved. Every project owns a folder, so a sound
// inside one belongs to that project; any other sound is in the Library only.

import { el, modal, notice } from './ui.js';
import { lib } from './native.js';
import { state } from './store.js';
import { baseOf } from './names.js';

/** @typedef {import('./search.js').Item} Item */
/** @typedef {import('./search.js').Project} Project */
/** @typedef {import('./search.js').Folder} Folder */

/** @param {string | null | undefined} path @returns {Project | null} */
export const projectOf = (path) => (path && state.projects.find(p => p.folder && path.startsWith(p.folder + '/'))) || null;

/** @param {Item} it */
export function placeChip(it) {
  const p = projectOf(it.path);
  return el('span', { class: 'place' + (p ? ' proj' : ''), 'data-tip': p ? `This file is in the folder of the project ${p.name}` : 'This file is in a Library folder. Athar never changes it' }, p ? `Project: ${p.name}` : 'Library');
}
/** @param {Item} it */
export const originChip = (it) => it.origin ? el('span', { class: 'place from', 'data-tip': `Made from ${it.origin}. That file is unchanged` }, `From ${baseOf(it.origin)}`) : null;

/** Each project as an option of a dialog's menu. @param {Project[]} projects @returns {[string, string][]} */
export const projectOptions = (projects) => projects.map(p => [`p:${p.id}`, `Project: ${p.name}`]);

/**
 * Asks for the name of a new sound and where it goes: a project that has a folder, or a Library
 * folder that is connected. `last` is the place picked the time before, offered again while it
 * still exists. Resolves null when cancelled, or when there is nowhere to save (`nowhere` says so).
 * @param {string} title @param {string} name @param {string | undefined} last @param {string} nowhere
 * @returns {Promise<{ name: string, dir: string, project: Project | null, place: string } | null>}
 */
export async function askSavePlace(title, name, last, nowhere) {
  /** @type {[Folder[], Project[]]} */
  const [folders, projects] = await Promise.all([lib.folders(), lib.projects()]);
  /** @type {[string, string][]} */
  const options = [...projectOptions(projects.filter(p => p.folder)), ...folders.filter(f => !f.offline).map(f => /** @type {[string, string]} */ ([`f:${f.path}`, `Library: ${baseOf(f.path)}`]))];
  if (!options.length) { notice(nowhere); return null; }
  const v = await modal(title, [{ label: 'Name', value: name }, { label: 'Into', options, value: options.some(o => o[0] === last) ? last : undefined }]);
  if (!v || !v[0]) return null;
  const project = projects.find(p => `p:${p.id}` === v[1]) || null;
  return { name: v[0], dir: project ? /** @type {string} */ (project.folder) : v[1].slice(2), project, place: v[1] };
}
