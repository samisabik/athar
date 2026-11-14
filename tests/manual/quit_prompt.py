"""Checks that closing with unsaved edits asks in the page's own dialog. Opens a window, so it is not part of pytest.

Run: ATHAR_HOME=<scratch dir> uv run python tests/manual/quit_prompt.py <folder with a .wav in it>
"""
import json
import os
import sys
import threading
import time

import AppKit
import uvicorn
import webview
from PyObjCTools import AppHelper
from webview.platforms.cocoa import BrowserView

from athar import server as srv
from athar.app import Api, free_port

out = {}


def js(w, code):
    return w.evaluate_js(code)


def ask_to_close(w):
    """Press the window's close button and report what the page and the system show."""
    if os.environ.get("QUIT_VIA") == "terminate":
        AppHelper.callAfter(AppKit.NSApp.terminate_, None)
    else:
        AppHelper.callAfter(BrowserView.instances[w.uid].window.performClose_, None)
    time.sleep(1.5)
    if w.uid not in BrowserView.instances:
        return {"closed": True}
    return {"closed": False, "system_alert": AppKit.NSApp.modalWindow() is not None,
            "title": js(w, "document.querySelector('#modal').hidden ? null : document.querySelector('#modalTitle').textContent"),
            "button": js(w, "document.querySelector('#modalOk').textContent")}


def run(w, api):
    try:
        time.sleep(3)
        out["clean_at_start"] = api.dirty is False
        js(w, "window.__tune = %s" % ("true" if os.environ.get("TUNE_OPEN") else "false"))
        js(w, """(async () => {
            const $ = s => document.querySelector(s), sleep = ms => new Promise(r => setTimeout(r, ms));
            window.__step = 'waiting for results';
            for (let i = 0; i < 60 && !$('#results .qitem[data-i]'); i++) {
                $('#q').value = 'tone'; $('#q').dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter'})); await sleep(1000);
            }
            window.__step = 'opening';
            $('#results .qitem[data-i]').dispatchEvent(new MouseEvent('dblclick', {bubbles: true}));
            for (let i = 0; i < 100 && $('#editor').classList.contains('blank'); i++) await sleep(100);
            $('#editor [data-a=reverse]').click();
            if (window.__tune) { await sleep(300); window.athar.editor.fx.openTool('tune'); await sleep(2500); const r = [...document.querySelectorAll('#fx .sl')].find(x => x.firstChild.textContent === 'Note'); if (r) { const i = r.querySelector('input'); i.value = +i.value + 2; i.dispatchEvent(new Event('input')); i.focus(); } await sleep(600); }
            window.__step = 'edited: ' + $('#editor [data-f=info]').textContent + ' api:' + typeof window.pywebview?.api?.set_dirty;
        })()""")
        for _ in range(80):
            time.sleep(0.5)
            out["step"] = js(w, "window.__step")
            if str(out["step"]).startswith("edited"):
                break
        time.sleep(1)
        out["dirty_after_edit"] = api.dirty is True
        out["asked_in_page"] = ask_to_close(w)
        js(w, "document.querySelector('#modalCancel').click()")
        time.sleep(0.5)
        out["still_open_after_cancel"] = w.uid in BrowserView.instances and js(w, "document.querySelector('#modal').hidden")
        out["asked_again"] = ask_to_close(w)
        js(w, "document.querySelector('#modalOk').click()")
        time.sleep(1.5)
        out["closed_after_quit"] = w.uid not in BrowserView.instances
    except Exception as e:
        out["error"] = f"{type(e).__name__}: {e}"
    finally:
        if w.uid in BrowserView.instances:
            w.destroy()


def main():
    srv.library().add_folder(sys.argv[1])
    port = free_port()
    server = uvicorn.Server(uvicorn.Config(srv.app, host="127.0.0.1", port=port, log_level="warning"))
    threading.Thread(target=server.run, daemon=True).start()
    while not server.started:
        time.sleep(0.05)
    threading.Timer(90, lambda: (print(json.dumps(out, indent=1), flush=True), os._exit(2))).start()
    api = Api()
    w = api.window = webview.create_window("Athar quit prompt check", f"http://127.0.0.1:{port}/?token={srv.TOKEN}", js_api=api, width=1280, height=820)
    w.events.closing += api.on_closing
    webview.start(run, (w, api))
    print(json.dumps(out, indent=1), flush=True)
    os._exit(0 if not out.get("error") else 1)


if __name__ == "__main__":
    main()
