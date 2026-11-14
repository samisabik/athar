"""Drives the new editor treatments in the real window. Run tests/manual/serve.py first, then:
uv run python tests/manual/drive_treatments.py <the URL it printed>"""
import json, sys, time, webview
JS = r"""
(async () => {
  const R = {errors: []}; window.__r = R;
  window.addEventListener('error', e => R.errors.push(String(e.message)));
  window.addEventListener('unhandledrejection', e => R.errors.push('rejection: ' + String(e.reason && e.reason.message || e.reason)));
  const $ = s => document.querySelector(s), sleep = ms => new Promise(r => setTimeout(r, ms));
  const until = async (f, ms = 20000) => { const t = Date.now(); while (Date.now() - t < ms) { const v = await f(); if (v) return v; await sleep(100); } return null; };
  const ed = window.athar.editor, set = (f, v) => { $(`#editor [data-f=${f}]`).value = v; };
  const info = () => $('#editor [data-f=info]').textContent;
  const open = async (name) => {
    await until(async () => { $('#q').value = name; $('#q').dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter'})); await sleep(900); return [...document.querySelectorAll('#results .qn')].some(n => n.textContent.includes(name)); });
    const row = [...document.querySelectorAll('#results .qitem[data-i]')].find(r => r.querySelector('.qn').textContent.includes(name));
    const v = ed.version; row.dispatchEvent(new MouseEvent('dblclick', {bubbles: true}));
    if (!$('#modal').hidden) $('#modalOk').click();
    await until(() => ed.version !== v && ed.name.includes(name)); await sleep(300);
  };
  const changed = async (act) => { const v = ed.version; act(); return until(() => ed.version !== v); };
  try {
    await until(() => $('#folders .item'));
    await open('hits_loop_120bpm');
    R.bpmPrefilled = $('#editor [data-f=bpm]').value;
    ed.action('transients'); R.hitRegions = await until(() => ed.regions.length);
    ed.regions[0].name = 'kick'; ed.renderRegions(); ed.persistRegions(); await sleep(500);
    R.regionChip = $('#editor [data-f=regions] [data-r=select]').textContent;
    ed.action('gridslice'); await sleep(200); R.gridRegions = ed.regions.length;
    ed.action('undo'); await sleep(600);
    await open('tone_220');
    await open('hits_loop_120bpm');
    R.regionsCameBack = await until(() => ed.regions.length && `${ed.regions.length} regions, first named ${ed.regions[0].name}`, 4000);

    await open('tone_220');
    R.before = await until(() => /LUFS/.test(info()) && info());
    set('lufs', -16); await changed(() => ed.action('loudnorm'));
    R.afterLoudnorm = await until(() => ed.meterVersion === ed.version && info());
    const len = ed.length;
    set('stretchlen', 150); set('semis', 0); await changed(() => ed.action('stretch'));
    R.stretchRatio = +(ed.length / len).toFixed(3);
    let z = 0; const d = ed.channels[0]; for (let i = 9600; i < d.length - 9600; i++) if ((d[i - 1] < 0) !== (d[i] < 0)) z++;
    R.pitchAfterStretch = +(z / 2 / ((d.length - 19200) / ed.sr)).toFixed(1);
    ed.action('undo');
    // a cut in the middle of a tone: the join must not jump
    const jump = (smooth) => { ed.settings.smooth_edits = smooth; ed.sel = [10000, 23467]; ed.action('delete'); const c = ed.channels[0]; let m = 0; for (let i = 9990; i < 10010; i++) m = Math.max(m, Math.abs(c[i] - c[i - 1])); ed.action('undo'); return +m.toFixed(4); };
    R.cutJumpRaw = jump(false); R.cutJumpSmoothed = jump(true);
    const l2 = ed.length; set('xfade', 50); ed.sel = null; ed.action('loopxfade'); await sleep(200);
    R.loopShorterBy = l2 - ed.length; R.loopOn = ed.loop; ed.stopPlay(); ed.action('undo');
    ed.channels[0][30000] += 0.9; ed.version++;
    set('clicksens', 5); await changed(() => ed.action('declick'));
    R.declick = $('#toast').textContent; R.clickGone = Math.abs(ed.channels[0][30000]) < 0.5;
    $('#editor [data-fx-open=eq]').click(); await sleep(300);
    $('#fx [data-preset="50"]').click(); await sleep(200); R.humBands = ed.fx.bands.map(b => Math.round(b.f));
    await changed(() => $('#fx [data-x=apply]').click());
    $('#editor [data-f=format]').value = 'flac';
    document.dispatchEvent(new KeyboardEvent('keydown', {key: 's', metaKey: true, bubbles: true}));
    R.savedAs = await until(() => info().includes('Saved') && ed.path);
    // batch over the two listed sounds, saved next to the originals
    document.querySelector('.ab[data-view=sounds]').click(); await sleep(300);
    $('#batchBtn').click(); await until(() => !$('#modal').hidden); R.batchTitle = $('#modalTitle').textContent;
    $('#modalOk').click();
    R.batch = await until(() => /Batch (done|stopped)/.test($('#toast').textContent) && $('#toast').textContent, 60000);
  } catch (e) { R.errors.push('test: ' + (e && e.stack || e)); }
  R.done = true;
})();
"""
def go(w):
    time.sleep(3)
    w.evaluate_js(JS)
    r = "{}"
    for _ in range(300):
        time.sleep(0.5)
        r = w.evaluate_js("JSON.stringify(window.__r || {})") or "{}"
        if json.loads(r).get("done"):
            break
    print(json.dumps(json.loads(r), indent=1), flush=True)
    w.destroy()
w = webview.create_window("Athar treatments check", sys.argv[1], width=1280, height=820)
webview.start(go, w)
