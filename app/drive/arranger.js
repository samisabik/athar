// The Arranger: two lanes play and loop, a muted lane goes quiet, and the mix is saved into the
// first Library folder. Needs a library with sounds named kick_44k* and tone_440*.
const { lib } = await import('/static/native.js');
const E = await import('/static/engine.js');
const lvl = async () => +(await E.invoke('audio_poll')).level.toFixed(3);
await E.setVolume(0.05);
const files = await lib.search({ sort: 'name', limit: 500 });
const by = (n) => files.find(f => f.name.startsWith(n));
const kick = by('kick_44k'), tone = by('tone_440');
await E.invoke('arr_set', { store: true, a: { loop: true, lanes: [
  { name: 'Kick', gain: 0, clips: [0, 0.5, 1, 1.5].map(at => ({ path: kick.path, duration: kick.duration, sr: kick.sr, at, in: 0, out: 0 })) },
  { name: 'Tone', gain: -6, clips: [{ path: tone.path, duration: tone.duration, sr: tone.sr, at: 0, in: 0.2, out: 0.5 }] },
] } });
document.querySelector('.ab[data-view="arranger"]').click(); await wait(2500);
const ar = window.athar.arranger;
log({ lanes: ar.a.lanes.length, length_s: ar.length(), loaded: [...ar.sounds.values()].filter(s => s.info).length });
await ar.start(); await wait(700);
const lv = []; for (let i = 0; i < 10; i++) { lv.push(await lvl()); await wait(100); }
log({ playing: !!ar.playing, levels: lv, position_s: +ar.position(ar.playing, E.now()).toFixed(2) });
await wait(1800);
log({ looped_position_s: +ar.position(ar.playing, E.now()).toFixed(2), still_playing: !!ar.playing });
ar.a.lanes[1].mute = true; ar.save(); await wait(400);
const muted = []; for (let i = 0; i < 6; i++) { muted.push(await lvl()); await wait(100); }
log({ tone_muted_levels: muted });
ar.stop(); await wait(300);
log({ stopped_level: await lvl() });
const dest = (await lib.folders())[0].path;
const r = await lib.saveMix({ rate: 48000, bits: 16 }, { dest_dir: dest, name: 'drive mix.wav' });
log({ mix_saved: r.path.split('/').pop(), over: r.over });
log('DONE');
