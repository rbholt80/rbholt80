"""Local web front end: one page, server-sent events, same engine as the CLI.

Bound to localhost and gated on a per-run token. The page is a static asset;
all state lives in the engine, so the browser and the terminal behave
identically.
"""

from __future__ import annotations

import json
import os
import queue
import secrets
import threading
import webbrowser
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from .engine import Roundtable
from .config import blocked, paint, discover

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
        self.remaining = 0
        self.active: dict | None = None
        self.connection_notes = blocked()
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
            if event["type"] == "start":
                self.active = {**event, "text": ""}
            elif event["type"] == "chunk" and self.active:
                self.active["text"] += event["text"]
            elif event["type"] == "end":
                self.active = None
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
            "remaining": self.remaining,
            "active": dict(self.active) if self.active else None,
            "connections": self.connection_notes,
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
                if event["type"] == "end" and event.get("error"):
                    self.set_auto(False)
        finally:
            self.turn_lock.release()
        return True

    def _driver(self) -> None:
        """Background loop: keeps the table talking while auto is on."""
        while not self.stopping.is_set():
            if not self.auto.wait(timeout=0.25):
                continue
            if self.auto.is_set() and self.run_one():
                self.remaining = max(0, self.remaining - 1)
                if not self.remaining:
                    self.set_auto(False)
            # A readable beat, but bail out immediately if auto is switched off.
            self.stopping.wait(self.pause)

    def say(self, text: str) -> None:
        turn = self.table.add_host_message(text)
        self.broadcast({"type": "turn", "speaker": turn.speaker,
                        "text": turn.text, "hex": "#e6e6e6", "error": False})

    def set_auto(self, on: bool) -> None:
        if on:
            self.remaining = len(self.table.participants)
        else:
            self.remaining = 0
        self.auto.set() if on else self.auto.clear()
        self.broadcast({"type": "auto", "on": on})

    def new_topic(self, topic: str) -> bool:
        if not self.turn_lock.acquire(blocking=False):
            return False
        try:
            self.set_auto(False)
            self.table.export_markdown()
            seats = paint(discover())
            if not seats:
                raise ValueError("No connected participants found")
            self.table = Roundtable(topic, seats,
                transcript_dir=self.table.transcript_path.parent,
                context_turns=self.table.context_turns)
            self.connection_notes = blocked()
            self.broadcast(self.snapshot())
            return True
        finally:
            self.turn_lock.release()


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
            self.send_header("Cache-Control", "no-store")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.end_headers()
            self.wfile.write(body)

        def _json(self, payload: dict, code: int = 200) -> None:
            self._send(code, json.dumps(payload).encode(), "application/json")

        def _body(self) -> dict:
            try:
                length = int(self.headers.get("Content-Length") or 0)
            except ValueError:
                raise ValueError("Invalid request length")
            if length <= 0 or length > MAX_BODY:
                self.close_connection = True
                raise ValueError("Request must contain a JSON object smaller than 64 KB")
            try:
                body = json.loads(self.rfile.read(length))
            except ValueError:
                raise ValueError("Invalid JSON")
            if not isinstance(body, dict):
                raise ValueError("Expected a JSON object")
            return body

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
            try:
                body = self._body()
            except ValueError as exc:
                self._json({"error": str(exc)}, 400)
                return

            if url.path == "/say":
                text = body.get("text", "")
                if not isinstance(text, str):
                    self._json({"error": "Message must be text"}, 400)
                    return
                text = text.strip()
                if text:
                    hub.say(text)
                self._json({"ok": bool(text)})
            elif url.path == "/next":
                name = body.get("speaker")
                if name is not None and (not isinstance(name, str) or not hub.table.by_name(name)):
                    self._json({"error": "Unknown participant"}, 400)
                    return
                if hub.turn_lock.locked():
                    self._json({"error": "A participant is still speaking"}, 409)
                    return
                started = threading.Thread(
                    target=hub.run_one, args=(body.get("speaker"),), daemon=True)
                started.start()
                self._json({"ok": True})
            elif url.path == "/auto":
                if not isinstance(body.get("on"), bool):
                    self._json({"error": "on must be true or false"}, 400)
                    return
                hub.set_auto(bool(body.get("on")))
                self._json({"ok": True, "auto": hub.auto.is_set()})
            elif url.path == "/new":
                topic = body.get("topic")
                if not isinstance(topic, str) or not topic.strip() or len(topic) > 2000:
                    self._json({"error": "Enter a topic between 1 and 2000 characters"}, 400)
                elif hub.new_topic(topic.strip()):
                    self._json({"ok": True})
                else:
                    self._json({"error": "Pause and let the current speaker finish first"}, 409)
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

    url = f"http://{host}:{httpd.server_port}/#{token}"
    if state_path := os.environ.get("ROUNDTABLE_STATE_PATH"):
        path = Path(state_path)
        with open(path, "w", opener=lambda name, flags: os.open(name, flags, 0o600)) as fh:
            json.dump({"pid": os.getpid(), "url": url}, fh)
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
        httpd.server_close()
        if table.history:
            print(f"Transcript: {table.export_markdown()}")
            print(f"Raw log:    {table.transcript_path}")
    return 0
