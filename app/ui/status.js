// The bottom bar's standing line: how far the analysis of new sounds has got, or a problem that
// lasts (the model did not load, there is no sound output).

import { $ } from './ui.js';
import { lib } from './native.js';
import { onOutputError, listen } from './engine.js';
import { state } from './store.js';
import { baseOf, plural } from './names.js';
import { search } from './search.js';
import { loadCats, loadFolders } from './sidebar.js';

// A standing problem takes the place of the progress until it is gone. Each source has its own
// text, '' for none; the first one set is shown.
const problems = { status: '', model: '', output: '' };
const problem = () => problems.status || problems.model || problems.output;

/** @param {boolean} on */
function showBar(on) { $('#indexing').hidden = !on; document.body.classList.toggle('indexing', on); }
function showProblem() {
  showBar(true);
  $('#indexingText').textContent = problem(); $('#indexingBar').hidden = true; $('#indexing .spin').hidden = true;
}

/** What the library's status says: the model, and the analysis queue.
    @typedef {{ model: string, error?: string | null, indexed: number, queued: number, done: number, pending_jobs: number, folder?: string | null }} Status */
let wasBusy = false;
/** @type {string | null} */
let lastModel = null;
/** @param {Status} s */
function showStatus(s) {
  problems.model = s.model === 'error' ? `Model failed to load: ${s.error}` : '';
  const about = $('#statusRight');
  if (about) about.textContent = `${plural(s.indexed, 'sound')} indexed`;
  // Analysis goes on in the background on every page; this says how far along it is.
  const left = s.queued && s.done < s.queued ? s.queued - s.done : 0, busy = !!(left || s.pending_jobs), starting = s.model === 'loading';
  if (problem()) showProblem();
  else {
    showBar(busy || starting);
    $('#indexingText').textContent = starting ? 'Starting up' : left ? `Analysing ${s.folder ? baseOf(s.folder) : 'new sounds'}` : 'Looking for new sounds';
    const bar = $('#indexingBar'), pct = left ? Math.round(s.done / s.queued * 100) : 0;
    bar.hidden = !left || starting;
    bar.firstElementChild.style.width = `${pct}%`;
    bar.setAttribute('aria-valuenow', pct);
    $('#indexing .spin').hidden = !bar.hidden;
  }
  // The kinds of sound are named by the model, so they are read again once it is ready.
  if (s.model !== lastModel) { lastModel = s.model; loadCats().catch(() => { }); }
  // The lists follow the analysis; the search is run again for the first sounds and when the work ends.
  if (s.indexed !== state.indexed || (!busy && wasBusy)) {
    const first = state.indexed <= 0;
    state.indexed = s.indexed;
    loadFolders().catch(() => { }); loadCats().catch(() => { });
    if (first || !busy) search();
  }
  wasBusy = busy;
}

/** @type {Status | null} */
let last = null;
/** Shows the library's status now, and again whenever the native side says it changed. */
export async function watchStatus() {
  listen('status', (/** @type {Status} */ s) => { last = s; problems.status = ''; showStatus(s); });
  try { const s = await lib.status(); if (!last) { last = s; showStatus(s); } }
  catch (e) { problems.status = `Could not read the library's status: ${e instanceof Error ? e.message : e}`; showProblem(); }
}

// The engine says when the sound output is lost and when it is back.
onOutputError((error) => {
  problems.output = error ? `No sound output: ${error}` : '';
  if (last) showStatus(last); else if (problem()) showProblem();
});
