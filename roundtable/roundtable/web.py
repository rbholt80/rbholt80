"""Local web front end: one page, server-sent events, same engine as the CLI.

Bound to localhost and gated on a per-run token. The page is a static asset;
all state lives in the engine, so the browser and the terminal behave
identically.
"""

from __future__ import annotations

import json
import queue
import secrets
import threading
import webbrowser
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from .engine import Roundtable

STATIC = Path(__file__).parent / "static"
MAX_BODY = 64 * 1024


class Hub:
    """Fan-out of engine events to connected browsers, plus the turn driver."""

    def __init__(self, table: Roundtable, token: str) -> None:
        self.table = table
        self.token = token
        self.subscribers: list[queue.Queue] = []
        self.lock = threading.Lock()          # guards subscriber list
        self.turn_lock = threading.Lock()     # only one speaker at a time
        self.auto = threading.Event()
        self.stopping = threading.Event()
        self.pause = 1.2
        threading.Thread(target=self._driver, daemon=True).start()

    # --- pub/sub ------------------------------------------------------------

    def subscribe(self) -> queue.Queue:
        q: queue.Queue = queue.Queue()
        with self.lock:
            self.subscribers.append(q)
        return q

    def unsubscribe(self, q: queue.Queue) -> None:
        with self.lock:
            if q in self.subscribers:
                self.subscribers.remove(q)

    def broadcast(self, event: dict) -> None:
        with self.lock:
            targets = list(self.subscribers)
        for q in targets:
            q.put(event)

    def snapshot(self) -> dict:
        return {
            "type": "snapshot",
            "topic": self.table.topic,
            "participants": [
                {"name": p.name, "hex": p.hex, "model": p.model, "kind": p.kind}
                for p in self.table.participants
            ],
            "history": [
                {"speaker": t.speaker, "text": t.text, "error": t.error,
                 "hex": self._hex(t.speaker)}
                for t in self.table.history
            ],
            "auto": self.auto.is_set(),
        }

    def _hex(self, speaker: str) -> str:
        p = self.table.by_name(speaker)
        return p.hex if p else "#8a8a8a"

    # --- driving ------------------------------------------------------------

    def run_one(self, speaker_name: str | None = None) -> bool:
        """Run a single turn if nobody is mid-sentence. False if busy."""
        if not self.turn_lock.acquire(blocking=False):
            return False
        try:
            speaker = (self.table.by_name(speaker_name) if speaker_name
                       else self.table.next_speaker())
            if speaker is None:
                return False
            for event in self.table.run_turn(speaker):
                self.broadcast(event)
        finally:
            self.turn_lock.release()
        return True

    def _driver(self) -> None:
        """Background loop: keeps the table talking while auto is on."""
        while not self.stopping.is_set():
            if not self.auto.wait(timeout=0.25):
                continue
            self.run_one()
            # A readable beat, but bail out immediately if auto is switched off.
            self.stopping.wait(self.pause)

    def say(self, text: str) -> None:
        turn = self.table.add_host_message(text)
        self.broadcast({"type": "turn", "speaker": turn.speaker,
                        "text": turn.text, "hex": "#e6e6e6", "error": False})

    def set_auto(self, on: bool) -> None:
        self.auto.set() if on else self.auto.clear()
        self.broadcast({"type": "auto", "on": on})


def _handler_factory(hub: Hub):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        server_version = "Roundtable"

        def log_message(self, *_args) -> None:  # keep the console for the chat
            pass

        # --- helpers --------------------------------------------------------

        def _authed(self, params: dict) -> bool:
            supplied = (params.get("t", [None])[0]
                        or self.headers.get("X-Roundtable-Token"))
            return bool(supplied) and secrets.compare_digest(supplied, hub.token)

        def _send(self, code: int, body: bytes, ctype: str) -> None:
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def _json(self, payload: dict, code: int = 200) -> None:
            self._send(code, json.dumps(payload).encode(), "application/json")

        def _body(self) -> dict:
            length = int(self.headers.get("Content-Length") or 0)
            if length <= 0 or length > MAX_BODY:
                return {}
            try:
                return json.loads(self.rfile.read(length) or b"{}")
            except ValueError:
                return {}

        # --- routes ---------------------------------------------------------

        def do_GET(self) -> None:  # noqa: N802
            url = urlparse(self.path)
            params = parse_qs(url.query)

            if url.path == "/":
                html = (STATIC / "index.html").read_bytes()
                self._send(200, html, "text/html; charset=utf-8")
                return

            if not self._authed(params):
                self._json({"error": "bad token"}, HTTPStatus.FORBIDDEN)
                return

            if url.path == "/state":
                self._json(hub.snapshot())
                return

            if url.path == "/events":
                self._stream()
                return

            self._json({"error": "not found"}, HTTPStatus.NOT_FOUND)

        def do_POST(self) -> None:  # noqa: N802
            url = urlparse(self.path)
            if not self._authed(parse_qs(url.query)):
                self._json({"error": "bad token"}, HTTPStatus.FORBIDDEN)
                return
            body = self._body()

            if url.path == "/say":
                text = (body.get("text") or "").strip()
                if text:
                    hub.say(text)
                self._json({"ok": bool(text)})
            elif url.path == "/next":
                started = threading.Thread(
                    target=hub.run_one, args=(body.get("speaker"),), daemon=True)
                started.start()
                self._json({"ok": True})
            elif url.path == "/auto":
                hub.set_auto(bool(body.get("on")))
                self._json({"ok": True, "auto": hub.auto.is_set()})
            elif url.path == "/save":
                path = hub.table.export_markdown()
                self._json({"ok": True, "path": str(path)})
            else:
                self._json({"error": "not found"}, HTTPStatus.NOT_FOUND)

        def _stream(self) -> None:
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.send_header("Connection", "keep-alive")
            self.end_headers()

            q = hub.subscribe()
            try:
                self._emit(hub.snapshot())
                while not hub.stopping.is_set():
                    try:
                        event = q.get(timeout=15)
                    except queue.Empty:
                        # Comment frame: keeps proxies and the browser from
                        # deciding an idle table is a dead connection.
                        self.wfile.write(b": keepalive\n\n")
                        self.wfile.flush()
                        continue
                    self._emit(event)
            except (BrokenPipeError, ConnectionResetError):
                pass
            finally:
                hub.unsubscribe(q)

        def _emit(self, event: dict) -> None:
            payload = json.dumps(event, ensure_ascii=False)
            self.wfile.write(f"data: {payload}\n\n".encode())
            self.wfile.flush()

    return Handler


def serve(table: Roundtable, host: str = "127.0.0.1", port: int = 8765,
          open_browser: bool = True) -> int:
    token = secrets.token_urlsafe(16)
    hub = Hub(table, token)
    httpd = ThreadingHTTPServer((host, port), _handler_factory(hub))
    httpd.daemon_threads = True

    url = f"http://{host}:{port}/#{token}"
    print(f"\nRoundtable: {table.topic}")
    print("  " + ", ".join(p.name for p in table.participants))
    print(f"\n  {url}\n")
    print("  Ctrl+C to stop.\n")
    if open_browser:
        threading.Thread(target=webbrowser.open, args=(url,), daemon=True).start()

    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("\nstopping…")
    finally:
        hub.stopping.set()
        hub.auto.clear()
        httpd.shutdown()
        if table.history:
            print(f"Transcript: {table.export_markdown()}")
            print(f"Raw log:    {table.transcript_path}")
    return 0
