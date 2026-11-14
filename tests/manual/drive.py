import json, sys, time, webview
URL = sys.argv[1]
JS = r"""
(async () => {
  const R = {errors: []}; window.__r = R;
  window.addEventListener('error', e => R.errors.push(String(e.message)));
  window.addEventListener('unhandledrejection', e => R.errors.push('rejection: ' + String(e.reason && e.reason.message || e.reason)));
  const $ = s => document.querySelector(s), sleep = ms => new Promise(r => setTimeout(r, ms));
  const until = async (f, ms = 15000) => { const t = Date.now(); while (Date.now() - t < ms) { const v = f(); if (v) return v; await sleep(100); } return null; };
  const info = () => $('#editor [data-f=info]').textContent;
  try {
    const { run } = await import('/static/work.js');
    const sine = Float32Array.from({length: 48000}, (_, i) => Math.sin(i * 0.05));
    R.resampleLen = (await run('resample', {chs: [sine], from: 20000, to: 10000})).channels[0].length;
    R.wavBytes = (await run('render', {chs: [sine, sine], sr: 48000, rate: 44100, bits: 16, mono: true})).wav.byteLength;
    const search = async (q) => { $('#q').value = q; $('#q').dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter'})); await sleep(900); return [...document.querySelectorAll('#results .qitem[data-i]')]; };
    await until(() => $('#folders .item'), 20000);
    let rows = await search('stereo_44k');
    R.rows = rows.map(r => r.querySelector('.qn').textContent);
    const open = async (name) => { const r = (await search(name)).find(r => r.querySelector('.qn').textContent.includes(name)); r.dispatchEvent(new MouseEvent('dblclick', {bubbles: true})); };
    await open('stereo_44k');
    await until(() => !$('#editor').classList.contains('blank'));
    await sleep(300);
    R.oggInfo = info();
    $('#editor [data-a=view]').click();
    await sleep(2500);
    const cv = $('#wave'), px = cv.getContext('2d').getImageData(cv.width / 2, cv.height / 2, 1, 1).data;
    R.specPixel = [...px];
    $('#editor [data-a=view]').click();
    $('#editor [data-a=reverse]').click(); await sleep(200);
    R.afterEdit = info();
    open('tone_220'); 
    R.discardPrompt = await until(() => !$('#modal').hidden && $('#modalTitle').textContent);
    $('#modalCancel').click(); await sleep(200);
    R.stillOgg = $('#editor [data-f=title]').textContent;
    document.querySelector('.ab[data-view=editor]').click(); await sleep(200);
    $('#editor [data-a=speed]').click(); await sleep(200);
    $('#fx [data-x=apply]').click();
    await sleep(1500);
    R.afterSpeed = info();
    $('#editor [data-fx-open=comp]').click(); await sleep(300);
    $('#fx [data-x=apply]').click(); await sleep(1500);
    $('#editor [data-fx-open=nr]').click(); await sleep(300);
    $('#fx [data-x=apply]').click(); await sleep(2500);
    R.undoDepth = $('#editor [data-a=undo]').disabled ? 0 : 'has undo';
    // undo of an in-place edit restores every sample; the step holds only the touched span
    const ed = window.athar.editor, sum = () => ed.channels.reduce((a, d) => a + d.reduce((x, v, i) => x + v * ((i % 97) + 1), 0), 0);
    const before = sum(), len = ed.length;
    ed.sel = [1000, 5000]; ed.action('gain'); await sleep(100);
    R.gainChanged = sum() !== before;
    R.undoStepSamples = ed.undo.at(-1).channels[0].length;
    ed.action('undo'); R.undoExact = sum() === before && ed.length === len;
    ed.action('redo'); R.redoChanged = sum() !== before; ed.action('undo');
    ed.sel = null;
    R.peakChip = await until(() => /Peak|Clips/.test(info()) && info());
    // noise reduction across lanes
    if ($('#fx').hidden || !$('#fx [data-fx=nr]').classList.contains('on')) { $('#editor [data-fx-open=nr]').click(); await sleep(300); }
    let t0 = performance.now(); const v0 = ed.version;
    $('#fx [data-x=apply]').click(); await until(() => ed.version !== v0, 30000);
    R.nrMs = Math.round(performance.now() - t0);
    // a playback-style redraw reuses the layer
    t0 = performance.now(); for (let i = 0; i < 200; i++) ed.draw(); R.drawMs = +((performance.now() - t0) / 200).toFixed(3);
    R.clipChip = (ed.action('all'), $('#editor [data-f=gain]').value = 30, ed.action('gain'), await until(() => /Clips/.test(info()) && info()));
    ed.action('undo'); ed.sel = null;
    document.dispatchEvent(new KeyboardEvent('keydown', {key: 's', metaKey: true, bubbles: true}));
    R.saved = await until(() => info().includes('Saved') && info());
    R.toast = $('#toast').textContent;
  } catch (e) { R.errors.push('test: ' + (e && e.stack || e)); }
  R.done = true;
})();
"""
def go(w):
    time.sleep(3)
    w.evaluate_js(JS)
    for _ in range(120):
        time.sleep(0.5)
        r = w.evaluate_js("JSON.stringify(window.__r || {})")
        if r and json.loads(r).get("done"):
            break
    print(json.dumps(json.loads(r or "{}"), indent=1))
    w.destroy()
w = webview.create_window("Athar test", URL, width=1280, height=820)
webview.start(go, w)
