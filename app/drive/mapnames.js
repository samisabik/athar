const E = await import('/static/engine.js');
document.querySelector('.ab[data-view="map"]').click(); await wait(3000);
const out = [];
for (let L = 0; L <= 5; L++) out.push({ L, names: (await E.invoke('map_names', { level: L })).map(n => n.text) });
log(out); log('DONE');
