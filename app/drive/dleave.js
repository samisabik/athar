const E = await import('/static/engine.js'); const { lib } = await import('/static/native.js'); await E.setVolume(0.05);
const lvl = async () => +(await E.invoke('audio_poll')).level.toFixed(3);
document.querySelector('.ab[data-view="discover"]').click(); await wait(800);
const d = window.athar.discover;
d.q.value = 'ambient rain'; d.q.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); await wait(5000);
const it = d.items.find(i => i.url && i.duration > 60 && i.duration < 400); log({ title: it?.title, dur: it?.duration, items: d.items.length });
const t0 = performance.now(); d.play(it.url);
const seen = [];
for (let i = 0; i < 300 && (d.waiting || !d.voice); i++) { await wait(100); const b = document.querySelector('[data-fetching]'); if (b) seen.push(b.textContent); }
log({ start_s: +((performance.now() - t0) / 1000).toFixed(1), button_texts: [...new Set(seen)].slice(0, 8), playing: !!d.voice, level: await lvl() });
document.querySelector('.ab[data-view="sounds"]').click(); await wait(800);
log({ after_leave_level: await lvl() });
log('DONE');
