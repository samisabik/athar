// Discover: while a found sound plays, its outline fills up to where playback is. Needs the network.
const E = await import('/static/engine.js'); await E.setVolume(0.05);
document.querySelector('.ab[data-view="discover"]').click(); await wait(800);
const d = window.athar.discover;
d.q.value = 'kick drum'; d.q.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); await wait(6000);
const it = d.items.find(i => i.url && i.duration > 2 && i.duration < 20 && i.wave?.length) || d.items.find(i => i.url && i.duration > 2 && i.duration < 20);
log({ title: it.title, dur: it.duration, hasWave: !!it.wave?.length });
log({ visibility: document.visibilityState });
window.requestAnimationFrame = (f) => setTimeout(() => f(performance.now()), 16);
await d.play(it.url);
log({ voice: d.voice, same: d.playingUrl === it.url, minis: document.querySelectorAll('#discover .mini, .mini').length, withUrl: document.querySelectorAll('.mini[data-url]').length, played: document.querySelectorAll('.mini-played').length });
const fills = [];
for (let i = 0; i < 6; i++) { await wait(400); const n = document.querySelector('.mini[data-url] .mini-played'); fills.push(n ? n.style.clipPath : 'none'); }
log({ fills });
d.stop(); await wait(200); log({ after_stop: !!document.querySelector('.mini[data-url]') });
log('DONE');
