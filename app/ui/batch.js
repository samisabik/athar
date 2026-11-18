// Batch: prep or convert many sounds, each saved as a new file.

import { modal, notice, pickFolder } from './ui.js';
import { lib } from './native.js';
import { SETTINGS } from './settings.js';
import { dirOf, stemOf, plural } from './names.js';

/** @typedef {import('./search.js').Item} Item */

/** The batch that is running; asking for another one stops it. @type {{ stop: boolean } | null} */
let running = null;

/** @param {Item[]} items */
export async function batch(items) {
  if (running) { running.stop = true; return; }
  if (!items.length) return notice('There are no sounds listed to process');
  const v = await modal(`Batch: ${plural(items.length, 'sound')}`, [
    { label: 'Do', options: [['prep', 'Sampler prep: trim silence, normalize, edge fades'], ['convert', 'Convert the format only']] },
    { label: 'Save as', options: [['wav', 'WAV'], ['flac', 'FLAC'], ['mp3', 'MP3']], value: SETTINGS.export_format },
    { label: 'Where', options: [['beside', 'Next to each original'], ['folder', 'Into one folder']] },
  ]);
  if (!v) return;
  const [what, format, where] = v;
  const dir = where === 'folder' ? await pickFolder('Save the batch into') : null;
  if (where === 'folder' && !dir) return;
  const st = SETTINGS;
  const prep = what === 'prep' ? { silenceDb: st.silence_db, targetDb: st.prep_target_db, fadeInMs: st.prep_fade_in_ms, fadeOutMs: st.prep_fade_out_ms } : null;
  // A file saved next to its original takes the suffix of saved edits, so the two names differ.
  const suffix = dir ? '' : (st.edit_suffix || '_edit');
  const job = running = { stop: false };
  let saved = 0, failed = 0;
  for (const [i, it] of items.entries()) {
    if (job.stop) break;
    notice(`Batch: ${i + 1} of ${items.length}, ${it.name}. Press the batch button again to stop.`, 600000);
    try {
      await lib.saveBatch(it.path, { rate: st.export_rate || 0, bits: st.export_bits, mono: !!st.export_mono, prep },
        { dest_dir: dir || dirOf(it.path), name: `${stemOf(it.name)}${suffix}.${format}`, format, source: it.path });
      saved++;
    } catch { failed++; }
  }
  running = null;
  notice(`Batch ${job.stop ? 'stopped' : 'done'}: ${saved} saved${failed ? `, ${failed} failed` : ''}.`, 8000);
}
