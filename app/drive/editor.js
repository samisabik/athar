// The editor: playing and looping, edits with undo and redo, regions, a tool's audition, the
// compressor and the live EQ, the spectrogram, saving, and the preview player. Needs a library
// with a sound named tone_440*.
const { lib } = await import('/static/native.js');
const E = await import('/static/engine.js');
const inv = E.invoke, lvl = async () => +(await inv('audio_poll')).level.toFixed(3);
const errs = []; window.addEventListener('error', e => errs.push(String(e.message))); window.addEventListener('unhandledrejection', e => errs.push('rej: ' + String(e.reason?.message || e.reason)));
await E.setVolume(0.05);
const files = await lib.search({ sort: 'name', limit: 500 });
const tone = files.find(f => f.name.startsWith('tone_440'));
const ed = window.athar.editor;
document.querySelector('.ab[data-view="editor"]').click(); await wait(300);
await ed.load({ source: { path: tone.path }, name: tone.name, dir: '/tmp' });
await wait(800);
log({ loaded: ed.loaded, len: ed.length, sr: ed.sr, nch: ed.nch, layer: !!ed.layerKey, info: ed.$('[data-f=info]').textContent });
ed.loop = true; await ed.play(); await wait(600);
const p1 = ed.playPos(); await wait(500); const p2 = ed.playPos();
log({ playing: ed.playing, advanced_s: +((p2 - p1) / ed.sr).toFixed(2), level: await lvl() });
await wait(2200); log({ looping: ed.playing, level: await lvl() });
ed.stopPlay(); await wait(200); log({ stopped: await lvl() });
ed.loop = false;
// edits, undo, redo
ed.$('[data-f=gain]').value = -6; await ed.action('gain');
const pk = await inv('ed_analyse', { p: { kind: 'peak' } });
await ed.action('undo'); const pk0 = await inv('ed_analyse', { p: { kind: 'peak' } });
await ed.action('redo'); const pk1 = await inv('ed_analyse', { p: { kind: 'peak' } });
log({ gain_peak: +pk.toFixed(3), undo_peak: +pk0.toFixed(3), redo_peak: +pk1.toFixed(3), undo: ed.undoN, redo: ed.redoN });
ed.sel = [0, 48000]; await ed.action('trim'); log({ trimmed_len: ed.length, sel: ed.sel });
await ed.action('undo'); log({ after_undo_len: ed.length });
ed.sel = [0, 24000]; await ed.action('copy'); ed.sel = null; ed.cursor = 96000; await ed.action('paste'); log({ pasted_len: ed.length, sel: ed.sel });
await ed.action('undo');
ed.sel = null; ed.$('[data-f=speed]').value = 200; await ed.action('speed'); log({ speed200_len: ed.length });
await ed.action('undo');
// regions move with edits and come back with undo
ed.sel = [60000, 72000]; await ed.action('addregion'); ed.sel = [0, 24000]; await ed.action('delete');
log({ region_after_delete: ed.regions, len: ed.length, cursor: ed.cursor });
await ed.action('undo'); log({ region_after_undo: ed.regions, len: ed.length, sel: ed.sel });
await ed.action('clearregions'); ed.sel = null;
// tool audition
await ed.fx.openTool('speed'); ed.$('[data-f=speed]').value = 150; ed.fx.bypass = false;
await ed.audition('speed'); await wait(400); log({ auditioning: ed.auditioning, level: await lvl() });
ed.stopAudition(); await ed.fx.close();
// effect: compressor preview and apply
await ed.fx.open('comp'); ed.fx.comp.threshold = -30; ed.fx.comp.ratio = 8; ed.fx.paramsChanged(); ed.fx.bypass = false;
ed.draw(); await wait(1500);
log({ fx_view: ed.fx.displayView(), info: ed.fx.root.querySelector('.fx-info')?.textContent });
await ed.fx.apply(); log({ applied_undo: ed.undoN, peak_after: +(await inv('ed_analyse', { p: { kind: 'peak' } })).toFixed(3) });
await ed.fx.close();
// EQ live
await ed.fx.open('eq'); ed.fx.bands.push({ type: 'bell', f: 440, g: -18, q: 1, on: true }); ed.fx.bypass = false; ed.fx.bandChanged();
ed.cursor = 0; await ed.play(); await wait(700);
const eqLevel = +(await inv('audio_poll')).level.toFixed(4); ed.fx.drawEq(); await wait(400);
log({ eq_live: ed.fx.eqLive, eq_level: eqLevel, curves: !!ed.fx.curves, spectrum_peak: Math.max(...ed.fx.spectrum).toFixed(1) });
ed.stopPlay(); ed.fx.bands = []; ed.fx.touched = false; await ed.fx.close();
// spectrogram
await ed.action('view'); await wait(1500); log({ spec_tiles: ed.tiles?.map.size }); await ed.action('view');
// render
const dest = (await lib.folders())[0].path; const sv = await inv('ed_save', { p: { bits: 16, rate: 48000 }, meta: { dest_dir: dest, name: 'drive edit.wav' } }); log({ saved: sv.path.split('/').pop() });
// measure chip
await wait(800); log({ chips: ed.$('[data-f=info]').textContent });
// preview player
const { Player } = await import('/static/player.js'); const pl = new Player();
pl.src = tone.path; await pl.play(); await wait(500);
log({ preview_t: +pl.currentTime.toFixed(2), level: await lvl() }); pl.pause();
log({ errors: errs });
log('DONE');
