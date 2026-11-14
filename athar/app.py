import argparse
import json
import os
from pathlib import Path
import socket
import sys
import threading
import time
import webbrowser

import uvicorn
from athar import __version__


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Api:
    """Exposed to the page as window.pywebview.api."""

    window = None

    def pick_folder(self):
        import webview
        kind = getattr(getattr(webview, "FileDialog", None), "FOLDER", None) or webview.FOLDER_DIALOG
        res = self.window.create_file_dialog(kind)
        return res[0] if res else None

    def pick_file(self):
        import webview
        kind = getattr(getattr(webview, "FileDialog", None), "OPEN", None) or webview.OPEN_DIALOG
        res = self.window.create_file_dialog(kind)
        return res[0] if res else None

    dirty = False
    quitting = False
    asked = 0

    def quit_cancelled(self):
        self.asked = 0

    def set_dirty(self, dirty):
        self.dirty = bool(dirty)

    def quit_now(self):
        self.quitting = True
        self.window.destroy()

    def on_drop(self, event):
        """Folders dragged onto the window join the library."""
        from athar import server as srv
        added, problems = [], []
        for f in (event.get("dataTransfer") or {}).get("files", []):
            path = f.get("pywebviewFullPath")
            if not path:
                continue
            if not os.path.isdir(path):
                problems.append(f"{os.path.basename(path)} is a file. Drop the folder it is in.")
                continue
            try:
                added.append(os.path.basename(srv.library().add_folder(path)))
            except ValueError as e:
                problems.append(str(e))
        self.window.evaluate_js(f"window.atharDropped && window.atharDropped({json.dumps({'added': added, 'problems': problems})})")

    def watch_drops(self):
        from webview.dom import DOMEventHandler
        doc = self.window.dom.document
        # Only the drop is sent here. The page itself allows dropping (its own dragover listener
        # in main.js): a dragover handler on this side is called for every pointer move of every
        # drag, each time sending the whole event across, which froze the window while a sound
        # was dragged inside the Arranger.
        doc.events.drop += DOMEventHandler(self.on_drop, True, True)

    def on_closing(self):
        """With unsaved edits, closing is put on hold and the page asks in its own dialog.
        Its Quit button comes back through quit_now."""
        if not self.dirty or self.quitting:
            return True
        # A second request while the question is still up means the page is not answering:
        # let the app go instead of leaving no way out.
        if self.asked and time.time() - self.asked > 1.5:
            return True
        self.asked = time.time()
        ask = "window.atharConfirmQuit ? window.atharConfirmQuit() : window.pywebview.api.quit_now()"
        threading.Thread(target=lambda: self.window.evaluate_js(ask), daemon=True).start()
        return False


def mac_identity():
    """Dock name and icon when running as a plain Python process."""
    try:
        from AppKit import NSApplication, NSImage
        from Foundation import NSBundle
        info = NSBundle.mainBundle().infoDictionary()
        info["CFBundleName"] = "Athar"
        # WebKit only offers the microphone to a page when the app declares why it wants it.
        info["NSMicrophoneUsageDescription"] = "Athar records from your microphone or audio interface when you press Record."
        icon = Path(__file__).parent / "static" / "icon.png"
        image = NSImage.alloc().initWithContentsOfFile_(str(icon))
        if image is not None:
            NSApplication.sharedApplication().setApplicationIconImage_(image)
    except Exception:
        pass


def main():
    ap = argparse.ArgumentParser(description="Search local audio by how it sounds")
    ap.add_argument("--browser", action="store_true", help="open in the default browser instead of a window")
    ap.add_argument("--port", type=int, default=0)
    args = ap.parse_args()

    from athar import server as srv
    port = args.port or free_port()
    server = uvicorn.Server(uvicorn.Config(srv.app, host="127.0.0.1", port=port, log_level="warning"))
    t = threading.Thread(target=server.run, daemon=True)
    t.start()
    while not server.started:
        if not t.is_alive():
            sys.exit(f"Could not start the server on port {port}")
        time.sleep(0.05)
    url = f"http://127.0.0.1:{port}/?token={srv.TOKEN}"

    if args.browser:
        print(f"Athar running at {url}")
        webbrowser.open(url)
        try:
            t.join()
        except KeyboardInterrupt:
            pass
        return

    import webview
    mac_identity()
    webview.settings["ALLOW_DOWNLOADS"] = True
    api = Api()
    api.window = webview.create_window(f"Athar {__version__}", url, js_api=api, width=1280, height=820,
                                       min_size=(900, 600), background_color="#f7f2ea")
    api.window.events.closing += api.on_closing
    api.window.events.loaded += api.watch_drops
    webview.start(icon=str(Path(__file__).parent / "static" / "icon.png"))
    server.should_exit = True


if __name__ == "__main__":
    main()
