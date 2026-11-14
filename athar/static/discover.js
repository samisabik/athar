import { fmtTime } from './audio.js';

// Ideas for the dice. Archives match titles and tags, so these stay short and concrete.
const IDEAS = `
thunderstorm|distant thunder|rain on a tent|rain on a tin roof|hail|blizzard|wind in wires|wind through pines|howling wind|ice cracking|frozen lake|glacier|avalanche|geyser|lava|mud pot|hot spring|waterfall|creek|river rapids|ocean swell|sea cave|tide pool|shingle beach|underwater|hydrophone|dripping cave|sinkhole echo|desert night|sand dune|earthquake|volcano
dawn chorus|nightingale|owl|loon|raven|crows|seagulls|swifts|woodpecker|lyrebird|kookaburra|parrots|rooster|geese|swan wings|bats|frogs at night|tree frogs|bullfrog|toads|cicadas|crickets|katydids|bees|beehive|mosquito|flies|ants|whale|humpback song|dolphin clicks|orca|seal|walrus|sea lion|wolf howl|coyotes|fox scream|elk bugle|deer|moose|lion roar|elephant rumble|hyena|gibbon|howler monkey|horse hooves|cowbells|sheep|goats|pigs|camel|purring cat|dog pack|snake hiss|rattlesnake|alligator
church bell|bell tower|carillon|temple bell|singing bowl|gong|tam tam|wind chimes|music box|glass harmonica|waterphone|kalimba|mbira|hang drum|steel pan|balafon|marimba|vibraphone|celesta|toy piano|prepared piano|detuned piano|harpsichord|clavichord|pipe organ|harmonium|accordion|melodica|bagpipes|hurdy gurdy|nyckelharpa|didgeridoo|jaw harp|theremin|ondes martenot|musical saw|bowed cymbal|bowed vibraphone|cello harmonics|double bass|viola da gamba|erhu|guzheng|koto|shamisen|shakuhachi|sitar|tanpura|tabla|oud|ney|duduk|kora|ngoni|berimbau|charango|banjo|dobro|pedal steel|zither|autoharp|dulcimer|ocarina|pan flute|bass clarinet|contrabassoon|flugelhorn|tuba|alphorn|conch shell|taiko|frame drum|udu|cajon|talking drum|bodhran|timpani|snare rudiment|brushes on snare|rimshot|cowbell|woodblock|claves|guiro|cabasa|vibraslap|flexatone|ratchet|thunder sheet|rainstick|ocean drum
steam engine|steam whistle|train station|train passing|subway|tram|level crossing|freight yard|cable car|ferry horn|foghorn|ship engine room|sailboat rigging|harbor|lighthouse|airport|jet flyby|propeller plane|helicopter|glider|hot air balloon|rocket launch|sonic boom|motorcycle|scooter|tractor|bulldozer|chainsaw|lawnmower|bicycle bell|skateboard|ice skates|car door|car wash|windshield wipers|turn signal|gravel driveway|toll booth|parking garage
typewriter|dot matrix printer|modem|fax machine|rotary phone|dial tone|busy signal|answering machine|cassette rewind|tape hiss|vinyl crackle|record skip|radio tuning|short wave radio|numbers station|morse code|telegraph|geiger counter|sonar ping|radar|theremin sweep|tesla coil|arc welder|transformer hum|fluorescent light|neon sign|old television|crt whine|slide projector|film projector|camera shutter|flash charging|photocopier|cash register|adding machine|slot machine|pinball|arcade|jukebox|cuckoo clock|grandfather clock|metronome|stopwatch|sewing machine|loom|spinning wheel|printing press|sawmill|forge|anvil|blacksmith|foundry|glass blowing|pottery wheel|grinding stone|water wheel|windmill|oil pump|drill press|lathe|jackhammer|pile driver|rivet gun|air compressor|hydraulic press|conveyor belt|bottling plant|elevator|escalator|revolving door|vending machine|microwave|kettle|espresso machine|blender|dishwasher|washing machine|ceiling fan|refrigerator hum|ice cubes|sizzling pan|popcorn|soda can|cork pop|champagne|pouring water|bathtub drain|shower|toothbrush|zipper|velcro|scissors|paper tearing|bubble wrap|cardboard box|packing tape|pencil on paper|chalk on blackboard|page turning|book closing|keys|coins|marbles|dice|dominoes|chess clock|playing cards|matches|lighter|candle|fireplace|campfire|fireworks|sparkler|balloon|rubber band|squeaky toy|wind up toy|spinning top|slinky|yo-yo|creaking door|squeaky hinge|door slam|doorbell|door knocker|floorboards|staircase|rocking chair|bed springs|drawer|lock|padlock|bolt|chain|gate|fence|ladder|rope stretch|sail flapping|flag in wind|tent zipper|umbrella
market crowd|bazaar|auction|stadium chant|applause|laughter|whispering|children playing|schoolyard|lullaby|humming|whistling|yodel|throat singing|overtone singing|gregorian chant|call to prayer|choir warmup|opera warmup|beatbox|tongue click|breath|yawn|sneeze|heartbeat|footsteps on snow|footsteps on gravel|footsteps in mud|high heels|running upstairs|tap dance|flamenco|protest|parade|marching band|street musician|busker|playground swing|carousel|fairground|circus|rodeo|bowling alley|billiards|ping pong|tennis rally|basketball court|ice hockey|fencing|archery|golf swing|swimming pool|diving board|rowing|skiing|sled dogs
restaurant kitchen|cafe|pub|diner|library|museum|cathedral|monastery|mosque courtyard|temple|cemetery|hospital|dentist drill|laboratory|server room|data center|power station|boiler room|ventilation shaft|tunnel|sewer|stairwell|parking lot|rooftop|construction site|shipyard|scrapyard|quarry|mine shaft|oil rig|farm|barn|stable|greenhouse|vineyard|rice paddy|bamboo forest|rainforest|mangrove|swamp|savanna|tundra|mountain pass|alpine meadow|canyon echo|cave|abandoned factory|ghost town|empty pool|silo
drone|tape loop|feedback|contact microphone|electromagnetic|induction coil|hum|static|white noise|pink noise|sine sweep|test tone|glitch|circuit bending|data sonification|bit crushed|granular|reversed|slowed down|sped up|time stretch|vocoder|talkbox|ring modulator|spring reverb|plate reverb|tape echo|lo-fi|8-bit|chiptune|dial-up|sci-fi door|laser|robot|computer beep|alarm|siren|air raid siren|klaxon|buzzer|countdown|launch control|mission control|spacewalk|satellite|pulsar|solar wind|aurora|whistlers|jupiter|saturn rings|mars wind|moon landing|sputnik|radio telescope
wax cylinder|78 rpm|gramophone|old radio drama|radio jingle|newsreel|public announcement|station ident|test card|shipping forecast|time signal|speaking clock|telephone operator|dictation|field recording|sound walk|binaural|ambisonic|room tone|foley|impact|whoosh|riser|downer|stinger|braam|sub drop|808|909|breakbeat|amen break|drum fill|snare roll|hi-hat loop|shaker loop|conga loop|bongo|djembe|clap|finger snap|stomp|vocal chop|ad lib|shout|chant|one shot|bass stab|chord stab|orchestra hit|string swell|brass fall|harp glissando|piano cluster|organ drone|choir pad|synth arp|acid line|reese bass|hoover|supersaw|rave stab|dub siren|sound system|steel drum|gamelan|carnival|second line|mariachi|klezmer|fado|tango|bossa nova|highlife|afrobeat|qawwali|raga|tuvan|joik|sea shanty|work song|field holler|ring shout|spiritual|barbershop|doo wop|skiffle|surf guitar|spaghetti western|library music|exotica|musique concrete|tape music|radiophonic
`.trim().split(/[|\n]/).map(x => x.trim()).filter(Boolean);
const TWISTS = ['distant', 'close', 'slow', 'old', 'broken', 'tiny', 'huge', 'underwater', 'metal', 'wooden', 'glass', 'night', 'empty', 'crowded', 'rusty', 'wet', 'frozen', 'electric', 'toy', 'giant'];
const THINGS = ['bell', 'door', 'engine', 'drum', 'pipe', 'machine', 'room', 'street', 'forest', 'radio', 'clock', 'bridge', 'gate', 'fan', 'motor', 'whistle', 'horn', 'chain', 'spring', 'wire', 'tank', 'bowl', 'box', 'hall', 'market', 'harbor', 'factory', 'kitchen', 'train', 'piano'];
// Deals every idea once before any repeats, with a made-up pairing now and then.
let bag = [];
function nextIdea() {
  if (Math.random() < 0.15) return `${TWISTS[Math.floor(Math.random() * TWISTS.length)]} ${THINGS[Math.floor(Math.random() * THINGS.length)]}`;
  if (!bag.length) { bag = [...IDEAS]; for (let i = bag.length - 1; i > 0; i--) { const j = Math.floor(Math.random() * (i + 1)); [bag[i], bag[j]] = [bag[j], bag[i]]; } }
  return bag.pop();
}
const HELLOS = ['What have you never heard?', 'Go find something strange.', 'Dig somewhere new.', 'What is out there?', 'Borrow a sound from the world.', 'Somebody recorded this.', 'Find a sound nobody else has.'];
const PLACEHOLDER = 'Search free sound archives';
const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
const DEST_KEY = 'athar-discover-project', SOURCE_KEY = 'athar-discover-source';
const remember = (k, v) => { try { v == null ? localStorage.removeItem(k) : localStorage.setItem(k, v); } catch { } };
const recall = (k) => { try { return localStorage.getItem(k); } catch { return null; } };
const mb = (n) => n ? `${(n / 1048576).toFixed(n < 10485760 ? 1 : 0)} MB` : '';

/** Search of free online sound archives. A found sound plays from the archive; adding it
    downloads it into a library folder with its licence and origin in the note. */
export class Discover {
  constructor(root, h) {
    this.root = root; this.h = h;
    this.sources = []; this.source = recall(SOURCE_KEY) || 'openverse';
    this.query = ''; this.page = 1; this.items = []; this.more = false; this.seq = 0;
    this.open = new Map(); this.added = new Set(); this.busy = new Set();
    this.dest = recall(DEST_KEY);
    this.player = new Audio(); this.player.preload = 'none'; this.playingUrl = null;
    // Archives can take several seconds to start sending a file, so the button shows the wait.
    this.player.onplaying = () => { this.waiting = false; this.paint(); };
    this.player.onwaiting = () => { if (this.playingUrl) { this.waiting = true; this.paint(); } };
    this.player.onended = this.player.onerror = () => { if (this.player.error && this.playingUrl) this.h.notice('That sound could not be played from the archive'); this.playingUrl = null; this.paint(); };
    this.build();
  }

  build() {
    const { el, ib, icon } = this.h;
    this.hello = el('div', { class: 'hello', 'aria-hidden': 'true' });
    this.q = el('input', { placeholder: PLACEHOLDER, spellcheck: 'false', autocomplete: 'off', 'aria-label': 'Search free sound archives' });
    this.q.onkeydown = (e) => {
      if (e.key === 'Enter') this.search(this.q.value);
      // Tab takes the idea being typed out in the empty box.
      else if (e.key === 'Tab' && !e.shiftKey && !this.q.value && this.idea) { e.preventDefault(); this.q.value = this.idea; }
    };
    this.sourceSel = el('select', { 'aria-label': 'Archive' });
    this.sourceSel.onchange = () => { this.source = this.sourceSel.value; remember(SOURCE_KEY, this.source); this.query ? this.search(this.query) : this.paint(); };
    this.filters = el('div', { class: 'filters', hidden: '' }, el('label', { class: 'f' }, el('span', {}, 'Archive'), this.sourceSel));
    this.filterBtn = ib('tune', 'Filters', () => { const open = this.filters.hidden; this.filters.hidden = !open; this.filterBtn.setAttribute('aria-expanded', String(open)); this.filterBtn.classList.toggle('on', open); });
    this.filterBtn.setAttribute('aria-expanded', 'false');
    this.scope = el('span', { class: 'scope' });
    this.list = el('div', { class: 'list big disc-list' });
    this.root.querySelector('.disc-top').append(this.hello,
      el('div', { class: 'searchbar' }, el('span', { class: 'i lead' }, 'travel_explore'), this.q, this.filterBtn),
      this.filters,
      el('div', { class: 'scope-row' }, this.scope));
    this.root.querySelector('.stack').append(this.list);
  }

  async show() {
    // Asked every time, since a Freesound key added in Settings adds an archive.
    this.visible = true;
    this.sources = await this.h.api('/api/discover/sources');
    if (!this.sources.some(s => s.id === this.source)) this.source = this.sources[0].id;
    this.root.classList.remove('idle');
    this.paint();
  }

  // Added sounds always go into a project, downloaded into its folder. The last project
  // picked is remembered while it still exists.
  async checkDest() {
    this.projects = await this.h.api('/api/projects');
    if (!this.projects.some(p => String(p.id) === this.dest)) this.dest = null;
  }
  async chooseDest() {
    await this.checkDest();
    if (!this.projects.length) { this.h.notice('Create a project on the Projects page first. Found sounds are added to a project.'); return null; }
    const pick = await this.h.modal('Add found sounds to', [{ label: 'Project', options: this.projects.map(p => [String(p.id), p.folder ? p.name : `${p.name} (needs a folder)`]), value: this.dest || undefined }]);
    if (!pick) return null;
    const p = this.projects.find(x => String(x.id) === pick[0]);
    // The download has to land somewhere on disk, so a project without a folder gets one now.
    if (!p.folder && !await this.h.projectFolder(p)) return null;
    this.dest = pick[0]; remember(DEST_KEY, this.dest);
    await this.checkDest();
    this.paint();
    return this.dest;
  }

  // Before a search the box sits in the middle of the page, as in the Library.
  setIdle(idle) {
    if (this.root.classList.contains('idle') === idle) return;
    this.root.classList.toggle('idle', idle);
    clearTimeout(this.typing);
    if (!idle) { this.q.placeholder = PLACEHOLDER; this.idea = null; return; }
    this.hello.textContent = HELLOS[Math.floor(Math.random() * HELLOS.length)];
    const any = nextIdea;
    if (reduceMotion || !this.visible) { this.q.placeholder = this.idea = any(); return; }
    let phrase = '', n = 0, wiping = false;
    const step = () => {
      if (!wiping && n === 0) phrase = this.idea = any();
      n += wiping ? -2 : 1;
      if (n >= phrase.length) { n = phrase.length; wiping = true; this.q.placeholder = phrase; this.typing = setTimeout(step, 1900); return; }
      if (n <= 0) { n = 0; wiping = false; }
      this.q.placeholder = phrase.slice(0, n) || ' ';
      this.typing = setTimeout(step, wiping ? 18 : 55 + Math.random() * 50);
    };
    step();
  }


  async search(query, page = 1) {
    query = query.trim(); if (!query) return;
    const seq = ++this.seq;
    this.query = query; this.loading = true;
    if (page === 1) { this.items = []; this.open.clear(); this.stop(); this.cur = 0; }
    this.paint();
    try {
      const r = await this.h.api('/api/discover/search', { json: { source: this.source, query, page } });
      if (seq !== this.seq) return;
      this.items = page === 1 ? r.items : [...this.items, ...r.items];
      this.page = page; this.more = r.more;
    } catch (e) { if (seq === this.seq) this.h.fail(e); }
    finally { if (seq === this.seq) { this.loading = false; this.paint(); this.loadOutlines(); } }
  }

  async toggleFiles(it) {
    if (this.open.has(it.id)) { this.open.delete(it.id); return this.paint(); }
    this.open.set(it.id, null); this.paint();
    try {
      const seen = new Set();
      const files = (await this.h.api('/api/discover/files', { json: { source: this.source, id: it.id } })).filter(f => !seen.has(f.url) && seen.add(f.url));
      if (this.open.has(it.id)) this.open.set(it.id, files);
    } catch (e) { this.open.delete(it.id); this.h.fail(e); }
    this.paint();
  }

  play(url) {
    if (this.playingUrl === url) return this.stop();
    this.h.stopOthers?.();
    this.player.src = url; this.player.volume = this.h.volume();
    this.playingUrl = url; this.waiting = true;
    this.player.play().catch((e) => { if (this.playingUrl === url && e?.name !== 'AbortError') { this.h.notice('That sound could not be played from the archive'); this.stop(); } });
    this.paint();
  }
  stop() { if (!this.playingUrl) return; this.playingUrl = null; this.waiting = false; this.player.pause(); this.player.removeAttribute('src'); this.paint(); }
  setVolume(v) { this.player.volume = v; }
  /** Back to the landing: called when the page is left, so it opens fresh next time. */
  reset() {
    this.stop(); this.seq++; this.visible = false; clearTimeout(this.typing);
    this.query = ''; this.items = []; this.more = false; this.sort = null; this.loading = false; this.page = 1; this.open.clear();
    this.q.value = '';
    this.paint();
  }

  async add(it, file) {
    const url = file ? file.url : it.url;
    if (this.busy.has(url) || this.added.has(url)) return;
    // Asked every time, with the last project already picked, so Enter repeats the choice.
    const dest = await this.chooseDest();
    if (!dest) return;
    this.busy.add(url); this.paint();
    const src = this.sources.find(s => s.id === this.source)?.name || this.source;
    const note = [`${it.title}${it.creator ? ` by ${it.creator}` : ''}`, it.license, `From ${src}${it.via ? ` (${it.via})` : ''}: ${it.page}`].filter(Boolean).join('. ');
    try {
      const r = await this.h.api('/api/discover/save', { json: { url, title: file ? file.name : it.title, project: +dest, note, source: this.source } });
      this.added.add(url);
      this.h.notice(`Added ${r.path.split('/').pop()} to ${this.projects.find(p => String(p.id) === dest)?.name || 'the project'}`);
      this.h.onAdd?.();
    } catch (e) { this.h.fail(e); }
    finally { this.busy.delete(url); this.paint(); }
  }

  paint() {
    const { el, ib, icon } = this.h;
    if (this.sourceSel.options.length !== this.sources.length) this.sourceSel.replaceChildren(...this.sources.map(x => el('option', { value: x.id }, x.name)));
    this.sourceSel.value = this.source;
    this.scope.textContent = this.loading ? 'Searching' : this.items.length ? `${this.items.length}${this.more ? '+' : ''} found for "${this.query}"` : '';
    this.setIdle(!this.items.length && !this.loading && !this.query);

    const addBtn = (it, file) => {
      const url = file ? file.url : it.url;
      return this.added.has(url) ? ib('check', 'Added', () => { }, 'on')
        : ib(this.busy.has(url) ? 'hourglass_top' : 'add', this.busy.has(url) ? 'Downloading' : 'Add to a project', (e) => { e.stopPropagation(); this.add(it, file); });
    };
    const playBtn = (url) => ib(this.playingUrl !== url ? 'play_arrow' : this.waiting ? 'hourglass_top' : 'stop', this.playingUrl === url && this.waiting ? 'Waiting for the archive to send the sound' : this.playingUrl === url ? 'Stop' : 'Play', (e) => { e.stopPropagation(); this.play(url); });
    const rows = [];
    // Column names sort what has been loaded so far; a second click goes back to the archive's own order.
    const head = (label, key) => {
      const on = this.sort === key, flip = () => { this.sort = on ? null : key; this.cur = 0; this.paint(); };
      return this.h.focusable(el('span', { class: 'th sortable' + (on ? ' on' : ''), role: 'button', 'data-tip': on ? 'Back to best match first' : `Sort by ${label}`, onclick: flip }, label), flip);
    };
    if (this.items.length) rows.push(el('div', { class: 'disc-row disc-head' }, el('span'), el('span'), head('name', 'name'), head('length', 'duration'), el('span')));
    for (const it of this.shown()) {
      const multi = !it.url, files = this.open.get(it.id), opened = this.open.has(it.id);
      const act = multi ? () => this.toggleFiles(it) : () => this.play(it.url);
      rows.push(this.h.focusable(el('div', { class: 'disc-row' + (opened ? ' on' : '') + (this.shown()[this.cur] === it ? ' cur' : ''), onclick: () => { this.cur = this.shown().indexOf(it); act(); }, 'data-tip': multi ? 'Click to see the recordings inside' : null },
        multi ? ib(opened ? 'expand_more' : 'chevron_right', 'Show the recordings inside', (e) => { e.stopPropagation(); act(); }) : playBtn(it.url),
        this.outline(it),
        el('span', { class: 'qn' }, it.title, it.creator ? el('span', { class: 'qsub' }, it.creator) : null),
        el('span', { class: 'tc num' }, it.duration ? fmtTime(it.duration) : ''),
        multi ? el('span') : addBtn(it)), act));
      if (!opened) continue;
      if (!files) rows.push(el('div', { class: 'disc-file muted' }, el('span'), 'Looking inside'));
      else if (!files.length) rows.push(el('div', { class: 'disc-file muted' }, el('span'), 'No sound files Athar can read in this one'));
      else for (const f of files) rows.push(el('div', { class: 'disc-file', onclick: () => this.play(f.url) },
        playBtn(f.url), el('span', { class: 'qn' }, f.name),
        el('span', { class: 'tc num' }, [f.duration ? fmtTime(f.duration) : '', mb(f.size)].filter(Boolean).join('  ')), addBtn(it, f)));
    }
    if (!this.items.length && !this.loading && this.query) {
      rows.push(el('div', { class: 'empty' }, el('h3', {}, 'Nothing found'), el('p', { class: 'muted' }, 'Try other words, or another archive.')));
    }
    if (this.more && !this.loading) rows.push(el('button', { class: 'ghost disc-more', onclick: () => this.search(this.query, this.page + 1) }, icon('expand_more'), 'More results'));
    const top = this.list.scrollTop;
    this.list.replaceChildren(...rows);
    this.list.scrollTop = top;
  }

  // Openverse can draw a sound's outline. They are fetched a few at a time after the list shows.
  outline(it) {
    const { el } = this.h, box = el('span', { class: 'mini', 'aria-hidden': 'true' });
    if (it.wave?.length) {
      let d = ''; it.wave.forEach((m, i) => { const hgt = Math.max(1, m / 255 * 24); d += `M${i + 0.5} ${((24 - hgt) / 2).toFixed(1)}v${hgt.toFixed(1)}`; });
      box.innerHTML = `<svg viewBox="0 0 ${it.wave.length} 24" preserveAspectRatio="none"><path d="${d}"/></svg>`;
    }
    return box;
  }
  async loadOutlines() {
    if (this.source !== 'openverse' || this.outlining) return;
    this.outlining = true;
    const seq = this.seq;
    try {
      for (;;) {
        const next = this.items.filter(i => i.url && i.wave === undefined).slice(0, 3);
        if (!next.length || seq !== this.seq) break;
        await Promise.all(next.map(async (it) => { it.wave = null; try { it.wave = await this.h.api('/api/discover/waveform', { json: { source: 'openverse', id: it.id } }); } catch { } }));
        if (seq === this.seq) this.paint();
      }
    } finally { this.outlining = false; if (seq !== this.seq) this.loadOutlines(); }
  }

  // The results in the order on screen.
  shown() {
    if (!this.sort) return this.items;
    const by = this.sort === 'name' ? (a, b) => a.title.localeCompare(b.title, undefined, { sensitivity: 'base' }) : (a, b) => (a.duration ?? Infinity) - (b.duration ?? Infinity);
    return [...this.items].sort(by);
  }

  handleKey(e) {
    if (e.key === '/') { e.preventDefault(); this.q.focus(); return true; }
    const k = e.key.toLowerCase(), it = this.shown()[this.cur];
    if (e.metaKey || e.ctrlKey || e.altKey) return false;
    if (this.items.length && (k === 'j' || e.key === 'ArrowDown' || k === 'k' || e.key === 'ArrowUp')) {
      e.preventDefault();
      this.cur = Math.max(0, Math.min(this.items.length - 1, (this.cur || 0) + (k === 'j' || e.key === 'ArrowDown' ? 1 : -1)));
      this.paint(); this.list.querySelector('.disc-row.cur')?.scrollIntoView({ block: 'nearest' });
      return true;
    }
    if (it && k === ' ') { e.preventDefault(); it.url ? this.play(it.url) : this.toggleFiles(it); return true; }
    if (it && k === 'a' && it.url) { e.preventDefault(); this.add(it); return true; }
    if (e.key === 'Escape' || (e.key === ' ' && this.playingUrl)) { e.preventDefault(); this.stop(); return true; }
    return false;
  }
}
