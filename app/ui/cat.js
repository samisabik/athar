// The cat: it walks along the bottom bar and gives tips for the page you are on.

import { $, reduceMotion } from './ui.js';
import { CAT_SKINS } from './cats.js';
import { SECTION_LESSONS, PAGE_LESSONS } from './lessons.js';

/** @typedef {{ title: string, cards: string[] }} Lesson */
/** One sheet row and its frame sequence. @typedef {{ row: number, seq: number[], ms: number }} Anim */
/** @typedef {{ name: string, src: string, px: number, size: number, smooth: boolean, speed: number, anim: { [name: string]: Anim | undefined, right: Anim, idle: Anim } }} Skin */
const skins = /** @type {Record<string, Skin | undefined>} */ (CAT_SKINS);
const sectionLessons = /** @type {Record<string, Lesson | undefined>} */ (SECTION_LESSONS);
const pageLessons = /** @type {Record<string, Lesson | undefined>} */ (PAGE_LESSONS);

// Where the user is: the page, and the editor section open on it. Set by main.js.
export const where = { view: () => 'sounds', section: () => /** @type {string|null} */ (null) };

// Tips for the page the user is on.
/** @type {Record<string, string[] | undefined>} */
const CAT_TIPS = {
  sounds: [
    'Describe what you hear, like "rain on a tin roof". File names and tags count too.',
    'J and K move through the list. Space plays the sound.',
    'Press / to jump to the search box from anywhere on this page.',
    'Press 1 to 5 to rate the sound you are on. 0 clears it.',
    'Double-click a sound, or press E, to open it in the editor.',
    'Click the big name, or press R, to rename the file on disk.',
    'T jumps to tags, N to the note. Tags help you find field recordings later.',
    'Click name, length or rating at the top of the table to sort by it.',
    'Try searching for a feeling instead of a thing: "lonely metallic ping", "warm and dusty".',
    'Stuck on a beat? Search for something that should not fit, like "creaking door", and use it as percussion.',
    'The little waveform on each row shows the shape before you play it. Short spikes are hits, a long block is a texture.',
    'The stacked button above the list preps or converts every sound in the list in one go.',
    'Rate as you listen. Later, the filter button can show only your 4 and 5 dot sounds.',
    'Backspace moves the sound you are on to the Trash, after asking. Escape stops playback.',
    'The tile at the top right is the volume. Pull it left, off its shadow, for louder. Scroll works too.',
    'Write where and when in the note. In a year, "under the bridge, 6 am" will find it for you.',
  ],
  folders: [
    'Athar analyses every file in a folder once, then keeps watching it for new recordings.',
    'Tags, notes and ratings follow a file when you move or rename it in Finder.',
    'A folder that is missing, or on an SD card that is out, stays in the list, greyed until it is back.',
    'Add the folder your recorder imports into. New takes show up in the Library on their own.',
    'The refresh button rescans every folder, for when you changed files while Athar was closed.',
    'WAV, AIFF, FLAC, OGG, MP3 and M4A files are all read.',
    'Open a folder, then use its "Show its sounds in the Library" button to see only those sounds.',
    'Idea: keep one folder only for sounds you recorded yourself. It becomes your signature set.',
  ],
  discover: [
    'Search free archives here. No account needed.',
    'The empty search box keeps typing out ideas. Press Tab to take the one on screen, then Enter to search.',
    'The filter button in the search box picks the archive.',
    'Online search matches titles and tags only. Once a sound is in your library, you can find it by describing it.',
    'The plus button downloads a sound into a project. Its licence and source link are written into the note.',
    'Internet Archive results open to show the recordings inside. Click one to hear it.',
    'Old radio, wax cylinders and 78s on the Internet Archive are full of textures nobody else is using.',
    'Added sounds get the tag "discovered", so you can filter for them in the Library.',
    'Press / to jump to the search box.',
  ],
  map: [
    'The map is a landscape. Peaks are where many sounds that sound alike gather.',
    'Move over the land to see the sounds there. Zoom in and they all appear.',
    'Click a name on the map to light up that kind of sound and dim the rest. Click it again to clear.',
    'Rest the pointer on a dot to hear it. Sweep slowly to audition a whole neighbourhood.',
    'Scroll to zoom in on a crowd of dots. Drag to move around.',
    'Click a dot to keep it at the bottom, then send it to the editor.',
    'Look at the edges. Lonely dots far from any group are your strangest sounds.',
    'Two dots that sit together but come from different folders layer well.',
  ],
  recorder: [
    'Press R to start and stop a take.',
    'Takes stay in the list until you save or discard them. Nothing is written to disk before that.',
    'Watch the meter. If it fills to the end, the take clipped: move back or turn the input down.',
    'Open a take in the editor first to trim it, then save from there.',
    'Record the room with nobody in it. Room tone under a dry sample makes it sit in a mix.',
    'Tap on things. A table, a radiator and a mug are a drum kit nobody else has.',
    'Save straight into a project to fill it as you record.',
  ],
  arranger: [
    'The browser on the left is how sounds get here. Pick a project, then drag a sound onto a lane. Double-click a clip to edit its sound.',
    'A lane is a track you name: Drums, Vocal, Synth. Double-click its name to change it. Any sound can go in any lane.',
    'Drop a sound below the last lane and it starts a new lane. "Add a lane" makes an empty one to fill.',
    'Drag a clip sideways to move it in time, and up or down to move it to another lane. Hold alt to leave a copy behind.',
    'A moved clip catches on the start and end of the others, so two can sit end to end. Hold cmd to place it freely.',
    'Each clip has a small square at its top corners. Drag one inward to fade the clip in or out.',
    'For a crossfade, overlap two clips, then drag the fade out of the first and the fade in of the second across the overlap.',
    'Click a clip, press Cmd C, click the time ruler where you want it, then Cmd V.',
    'M mutes a lane. S solos it, so you hear it on its own.',
    'The ruler under each name is its level. Double-click it to go back to 0 dB.',
    'Pinch on the trackpad, or hold cmd and scroll, to zoom the lanes where the pointer is.',
    'Save mix writes what you hear as one new sound, into a project or a folder.',
    'Idea: layer a soft kick under a clicky one. Slide the soft one a few milliseconds later.',
    'Idea: put room tone under a dry hit, turn its lane down to -18 dB, and save the pair as one sound.',
    'Click the time ruler at the top to choose where playback starts.',
  ],
  projects: [
    'A project has its own folder. "Add to project" on a sound copies it in.',
    'Export makes an SP-404MKII project with the pads as laid out, ready for IMPORT PROJECT on the device.',
    'Double-click a sound in a project to open it in the editor.',
    'A sound can be added to several projects. Each gets its own copy, and the original stays in the Library.',
    'Exported files are named by the pattern in Settings, so they sort neatly on the sampler.',
    'The batch button on a project preps or converts every sound in it at once.',
    'Idea: build a project from one place. Eight sounds from the same kitchen already belong together.',
    'Idea: limit a project to 16 sounds, one per pad. Choosing is half the work.',
  ],
  editor: [
    'The magic wand preps a sample: trims silence, normalizes and adds tiny fades.',
    'Drag across the waveform to select, then R to add it as a region.',
    'Space plays and stops. L loops the selection.',
    'F fits the whole file. E zooms into the selection. Plus and minus zoom in and out.',
    'Hold cmd and scroll to zoom where the pointer is. Scroll alone moves along the sound.',
    'Shift-click extends the selection. Double-click the waveform selects everything. Escape clears it.',
    'Home and End jump to the start and the end.',
    'Fade has a ruler for each end. Select part of the waveform first and the fades sit at the ends of that part.',
    'Cmd T keeps only the selection. Cmd Z undoes it if you cut too close.',
    'Cmd S saves a new file and asks which project it goes into. The file you opened is never changed.',
    'Move any control and you hear the result. Turn Loop on first and it keeps playing while you adjust.',
    'The Original and Edited switch, or the B key, compares a change with the sound before it. Apply keeps the change.',
    'Auto-slice cuts a long recording at its silences. Each region saves as its own file.',
    'Slice at hits finds every drum hit in a loop. Then "Save each region" in File gives you one file per hit.',
    'Noise reduction works best if you first select a bit of pure background noise.',
    'Level, then "Normalize loudness", sets a LUFS target, so every sample in a project sits at the same level.',
    'Double-click a region\'s number to name it. Regions are remembered with the file.',
    'Stretch changes length without changing pitch, and pitch without changing length.',
    'File, then "Export for SP-404MKII", saves straight to the card in the right format.',
    'In the EQ, double-click to add a band, drag it to move, scroll on it to change its width.',
    'The spectrogram view shows pitch over time. Bird calls and squeaks are easy to spot there.',
    'Time, then "Tune to note", reads the pitch of an 808 or a one-shot and moves it onto the nearest note.',
    'Saturation makes a bass hit audible on small speakers. Push Drive, and the Ceiling keeps it from clipping.',
    'Shape has two rulers: Attack for more or less punch, Tail to shorten a ring or let it breathe.',
    'Lo-fi lowers bit depth and sample rate for old-sampler grit. Try 12-bit at 26 kHz on drums.',
    'Idea: set Speed to 50% on a field recording. Rain turns into a slow, low wash.',
    'Idea: reverse a cymbal or a door slam and end it right on a kick for a swell.',
    'Idea: select one second of room tone and use "Make it loop". You get a bed to sit under a beat.',
    'Idea: pitch a single hit up 12 semitones and down 12. That is three drums from one recording.',
    'Idea: stretch a half-second vocal to 400%. The smear is the point.',
  ],
  settings: [
    'Turn the cat off at the top of this page if I get in the way.',
    'Sampler prep settings change what the magic wand does in the editor.',
    'I have friends. Pick another cat from the list on this page.',
    'The accent colour changes every highlight and shadow in the app.',
    'Turn on play-as-you-move to hear each sound while you arrow through the Library.',
    'Smooth edits adds a tiny crossfade to every cut, so deletes do not click.',
    'Choose the format saved edits use by default: WAV, FLAC or MP3.',
    'The loudness target here is what "Normalize loudness" starts from in the editor.',
  ],
};
/** @type {{ x: number | null, moving: boolean, facing: number, stop: boolean, mode: string, t0: number }} */
const cat = { x: null, moving: false, facing: 1, stop: false, mode: 'idle', t0: 0 };
/** @type {Record<string, number>} */
const catTipIndex = {};
/** @type {Skin} */
let skin = CAT_SKINS.sukey;
const catImg = new Image();
// The cat lives in the band above the page: from its left edge to the volume ruler.
const CAT_MAX_SIZE = 72;
const catSize = () => Math.min(skin.size, CAT_MAX_SIZE);
const catMin = () => {
  const h = [...document.querySelectorAll('.vhead')].find(e => e.offsetParent);
  return h ? Math.round(h.getBoundingClientRect().left) + 28 : 90;
};
// The speech bubble reaches 86 px to the right of the cat, so the cat stops far enough from the
// volume tile for the bubble to clear it.
const catMax = () => Math.max(catMin(), Math.round($('.transport').getBoundingClientRect().left) - Math.max(catSize() + 12, 86 + 28));
const clampCat = (/** @type {number} */ x) => Math.min(Math.max(x, catMin()), catMax());
function catPlace() { $('#cat').style.transform = `translateX(${cat.x}px)`; }

/** @param {string} text */
function catSay(text) {
  const b = $('#catBubble'), left = (cat.x ?? 999) < 320;
  $('#catText').textContent = text;
  b.style.left = left ? '20px' : '-200px';
  b.classList.toggle('flip', left);
  b.hidden = false;
}
// What the cat can say where the user is. With an editor section open it explains that section
// first, then carries on with the page; on a page, its tips and its short explanations are mixed.
function catLines() {
  const view = where.view(), tips = CAT_TIPS[view] || /** @type {string[]} */ (CAT_TIPS.sounds), facts = pageLessons[view]?.cards || [];
  const mixed = [];
  for (let t = 0, f = 0; t < tips.length || f < facts.length;) {
    mixed.push(...tips.slice(t, t + 2)); t += 2;
    if (f < facts.length) mixed.push(facts[f++]);
  }
  const section = view === 'editor' ? where.section() : null;
  return section && sectionLessons[section] ? { key: section, list: [...sectionLessons[section].cards, ...mixed] } : { key: `page:${view}`, list: mixed };
}
/** @type {string | null} */
let catKey = null;
function catTip() {
  const { key, list } = catLines();
  const k = ((catTipIndex[key] ?? -1) + 1) % list.length;
  catTipIndex[key] = k; catKey = key;
  catSay(list[k]);
}
// Called when the open editor section changes: an open bubble starts on what that section does.
export function catContext() {
  if ($('#catBubble').hidden) return;
  const { key } = catLines();
  if (key !== catKey) { if (key in sectionLessons) catTipIndex[key] = -1; catTip(); }
}
window.addEventListener('athar:view', () => { $('#catBubble').hidden = true; });
$('#catBtn').onclick = () => { cat.stop = true; if ($('#catBubble').hidden) catTip(); else $('#catBubble').hidden = true; };
$('#catNext').onclick = catTip;
$('#catClose').onclick = () => { $('#catBubble').hidden = true; };
catContext();

/** @param {string} name */
export function setCatSkin(name) {
  const next = skins[name] || CAT_SKINS.sukey;
  if (next === skin && catImg.src) return;
  skin = next;
  const cv = $('#catCanvas');
  cv.width = cv.height = skin.px;
  cv.style.imageRendering = skin.smooth ? 'auto' : 'pixelated';
  $('#cat').style.setProperty('--cat-size', catSize() + 'px');
  catImg.onload = () => catShow('idle');
  catImg.src = skin.src;
  cat.mode = 'idle'; cat.t0 = performance.now();
  if (cat.x != null) { cat.x = clampCat(cat.x); catPlace(); }
}
// Skins with a dedicated left row draw it as-is; every other skin mirrors all its frames when facing left.
/** @param {string} name @param {number} i */
function catFrame(name, i) {
  const cv = $('#catCanvas');
  if (!cv || !catImg.complete || !catImg.naturalWidth) return;
  const a = skin.anim[name] || skin.anim.idle;
  const row = name === 'right' && cat.facing < 0 && skin.anim.left ? skin.anim.left.row : a.row;
  const f = a.seq[Math.min(i, a.seq.length - 1)];
  const g = cv.getContext('2d');
  g.clearRect(0, 0, skin.px, skin.px);
  g.drawImage(catImg, f * skin.px, row * skin.px, skin.px, skin.px, 0, 0, skin.px, skin.px);
  cv.style.transform = !skin.anim.left && cat.facing < 0 ? 'scaleX(-1)' : '';
}
/** @param {string} name */
function catShow(name) { cat.mode = name; cat.t0 = performance.now(); catFrame(name, 0); }
// Play a one-shot animation to its end, then return to idle.
/** @param {string} name @returns {Promise<void>} */
function catOnce(name) {
  const a = skin.anim[name];
  if (!a) return Promise.resolve();
  cat.mode = 'once'; cat.t0 = performance.now();
  return new Promise((done) => {
    let i = 0;
    const step = () => {
      if (cat.moving || cat.mode !== 'once') return done();
      catFrame(name, i);
      if (++i < a.seq.length) setTimeout(step, a.ms); else { catShow('idle'); done(); }
    };
    step();
  });
}

/** @type {number | undefined} */
let meowTimer;
function catMeow() {
  const m = $('#catMeow');
  m.classList.toggle('left', cat.facing < 0);
  m.hidden = false;
  clearTimeout(meowTimer);
  meowTimer = setTimeout(() => { m.hidden = true; }, 1400);
}

// The cat stays put most of the time: it sits, grooms or meows, then runs across to the other side.
/** @param {number} to @returns {Promise<void>} */
function catMove(to) {
  return new Promise((done) => {
    const from = /** @type {number} */ (cat.x), ms = Math.abs(to - from) / skin.speed * 1000, t0 = performance.now(), fm = skin.anim.right.ms;
    cat.facing = to < from ? -1 : 1; cat.moving = true; cat.stop = false;
    $('#cat').classList.add('moving');
    $('#catMeow').hidden = true;
    const step = (/** @type {number} */ t) => {
      const dt = Math.max(0, t - t0), k = ms ? Math.min(1, dt / ms) : 1;
      cat.x = clampCat(from + (to - from) * k); catPlace();
      catFrame('right', Math.floor(dt / fm) % skin.anim.right.seq.length);
      if (k < 1 && !cat.stop) { requestAnimationFrame(step); return; }
      cat.moving = false;
      $('#cat').classList.remove('moving');
      catOnce('sitdown').then(() => { if (cat.mode !== 'idle' && !cat.moving) catShow('idle'); });
      done();
    };
    requestAnimationFrame(step);
  });
}
async function catWander() {
  const busy = $('#cat').hidden || !$('#catBubble').hidden || reduceMotion;
  if (!busy) {
    if (Math.random() < .7) {
      const r = Math.random();
      if (r < .2) catMeow();
      else if (r < .4 && skin.anim.groom) await catOnce('groom');
      else if (r < .6 && !skin.anim.left) { cat.facing = -cat.facing; catShow('idle'); }
    } else {
      const lo = catMin(), hi = catMax(), mid = (lo + hi) / 2;
      const to = /** @type {number} */ (cat.x) > mid ? lo + Math.random() * (mid - lo) * .8 : hi - Math.random() * (hi - mid) * .8;
      await catMove(to);
      if (Math.random() < .25 && !cat.stop) catMeow();
    }
  }
  setTimeout(catWander, 1200 + Math.random() * 2300);
}
setInterval(() => {
  if ($('#cat').hidden || cat.moving || cat.mode === 'once') return;
  const a = skin.anim[cat.mode] || skin.anim.idle;
  catFrame(cat.mode, Math.floor((performance.now() - cat.t0) / a.ms) % a.seq.length);
}, 60);
setCatSkin('sukey');
cat.x = catMax(); catPlace();
window.addEventListener('resize', () => { cat.x = clampCat(/** @type {number} */ (cat.x)); catPlace(); });
window.addEventListener('athar:view', () => { if (!cat.moving) { cat.x = clampCat(/** @type {number} */ (cat.x)); catPlace(); } });
setTimeout(catWander, 1500);
