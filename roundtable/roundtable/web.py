"""Local web front end: one page, server-sent events, same engine as the CLI.

Bound to localhost and gated on a per-run token. The page is a static asset;
all state lives in the engine, so the browser and the terminal behave
identically.
"""

from __future__ import annotations

import copy
import json
import os
import queue
import secrets
import threading
import webbrowser
from http import HTTPStatus
from http.cookies import SimpleCookie
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from .engine import Roundtable
from .config import blocked, paint, discover

STATIC = Path(__file__).parent / "static"
MAX_BODY = 64 * 1024


class Hub:
    """Fan out committed engine events and drive one speaker at a time.

    A reconnect snapshots this event view, not the engine's mutable history:
    the engine may have persisted a reply immediately before yielding its end
    event. Registration and snapshot share the broadcast lock, so an event is
    always either in the snapshot or queued after it, never both.
    """

    def __init__(self, table: Roundtable, token: str) -> None:
        self.table = table
        self.token = token
        self.subscribers: list[queue.Queue] = []
        self.lock = threading.RLock()
        self.turn_lock = threading.Lock()
        self.stopping = threading.Event()
        self.wake = threading.Event()
        self.auto = threading.Event()
        self.pause = 1.2
        self.remaining = 0
        self._round_mode = False
        self.active: dict | None = None
        self._history = [t.as_event() for t in table.history]
        self._ledger = table.ledger()
        self.connection_notes = blocked()
        threading.Thread(target=self._driver, daemon=True).start()

    def subscribe(self) -> queue.Queue:
        q: queue.Queue = queue.Queue()
        with self.lock:
            self.subscribers.append(q)
        return q

    def subscribe_snapshot(self) -> tuple[queue.Queue, dict]:
        with self.lock:
            snapshot = self.snapshot()
            return self.subscribe(), snapshot

    def unsubscribe(self, q: queue.Queue) -> None:
        with self.lock:
            if q in self.subscribers:
                self.subscribers.remove(q)

    def broadcast(self, event: dict) -> None:
        with self.lock:
            kind = event["type"]
            if kind == "start":
                self.active = {**event, "text": ""}
            elif kind == "chunk" and self.active:
                self.active["text"] += event["text"]
            elif kind in ("turn", "end"):
                seq = event.get("seq")
                if seq is None or not any(t.get("seq") == seq for t in self._history):
                    self._history.append(dict(event))
                if kind == "end":
                    self.active = None
                    self._ledger = event.get("ledger", self.table.ledger())
            for q in self.subscribers:
                q.put(event)

    def snapshot(self) -> dict:
        with self.lock:
            return copy.deepcopy({
                "type": "snapshot", "topic": self.table.topic,
                "participants": [
                    {"name": p.name, "hex": p.hex, "model": p.model,
                     "kind": p.kind, "role": p.role, "note": p.note}
                    for p in self.table.participants
                ],
                "history": [{**t, "hex": self._hex(t["speaker"])}
                            for t in self._history],
                "running": self.running, "auto": self.running,
                "remaining": self.remaining,
                "active": self.active, "connections": self.connection_notes,
                "ledger": self._ledger,
            })

    def _hex(self, speaker: str) -> str:
        p = self.table.by_name(speaker)
        return p.hex if p else "#8a8a8a"

    @property
    def running(self) -> bool:
        return self.auto.is_set()

    def _owe(self, turns: int) -> None:
        with self.lock:
            self.remaining = max(0, turns)
            self.auto.set() if turns else self.auto.clear()
            self.broadcast({"type": "running", "on": self.running,
                            "remaining": self.remaining})
        self.wake.set()

    def set_auto(self, on: bool) -> bool:
        # The default is bounded: starting automatic discussion is one fair
        # round. Pause takes effect after the current reply is saved.
        if on:
            return self.run_round()
        with self.lock:
            self.table.clear_round()
            self._round_mode = False
            self._owe(0)
        return True

    def run_round(self, independent: bool = False) -> bool:
        if not self.turn_lock.acquire(blocking=False):
            return False
        try:
            with self.lock:
                if self.running:
                    return False
                names = self.table.queue_round(independent=independent)
                self._round_mode = True
                self._owe(len(names))
            return True
        finally:
            self.turn_lock.release()

    def _run_owned(self, speaker_name: str | None = None) -> bool:
        """The caller owns turn_lock and releases it after this returns."""
        speaker = (self.table.by_name(speaker_name) if speaker_name
                   else self.table.next_speaker())
        if speaker is None:
            return False
        for event in self.table.run_turn(speaker):
            self.broadcast(event)
            if event["type"] == "end" and event.get("error"):
                self.set_auto(False)
        return True

    def run_one(self, speaker_name: str | None = None) -> bool:
        if not self.turn_lock.acquire(blocking=False):
            return False
        try:
            return self._run_owned(speaker_name)
        finally:
            self.turn_lock.release()

    def start_one(self, speaker_name: str | None = None) -> bool:
        # Reserve synchronously before acknowledging the HTTP request; two
        # simultaneous clicks must not both receive an accepted response.
        if not self.turn_lock.acquire(blocking=False):
            return False
        if self.running:
            self.turn_lock.release()
            return False

        def worker():
            try:
                self._run_owned(speaker_name)
            finally:
                self.turn_lock.release()
        threading.Thread(target=worker, daemon=True).start()
        return True

    def _driver(self) -> None:
        while not self.stopping.is_set():
            if not self.running:
                self.wake.wait(timeout=0.25)
                self.wake.clear()
                continue
            if not self.turn_lock.acquire(timeout=0.1):
                continue
            ran = False
            try:
                with self.lock:
                    if self.running and self.remaining > 0:
                        self.remaining -= 1
                        ran = True
                if ran:
                    self._run_owned()
                    with self.lock:
                        if self.running and self._round_mode:
                            self.remaining = self.table.pending_round
                        if self.running and not self.remaining:
                            self._round_mode = False
                            self._owe(0)
            finally:
                self.turn_lock.release()
            if ran and self.running:
                self.stopping.wait(self.pause)

    def say(self, text: str) -> None:
        with self.lock:
            turn = self.table.add_host_message(text)
            self.broadcast({**turn.as_event(), "hex": "#e6e6e6"})

    def challenge(self, seq: int, quote: str, question: str) -> None:
        with self.lock:
            turn = self.table.add_challenge(seq, quote, question)
            self.broadcast({**turn.as_event(), "hex": "#e6e6e6"})

    def new_topic(self, topic: str) -> bool:
        if not self.turn_lock.acquire(blocking=False):
            return False
        try:
            self.set_auto(False)
            self.table.export_markdown()
            refresh = getattr(self.table, "refresh_participants", discover)
            seats = paint(refresh())
            if not seats:
                raise ValueError("No connected participants found")
            table = Roundtable(topic, seats,
                transcript_dir=self.table.transcript_path.parent,
                context_turns=self.table.context_turns,
                policy=self.table.policy,
                moderate_every=self.table.moderate_every)
            table.refresh_participants = refresh
            notes = blocked()
            with self.lock:
                self.table = table
                self._history = []
                self._ledger = table.ledger()
                self.active = None
                self.connection_notes = notes
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
            if not supplied:
                # A session cookie supports browsers that cannot retain
                # sessionStorage. POST also requires a non-simple custom
                # header, so another origin cannot submit cookie-only writes.
                if self.command == "POST" and self.headers.get("X-Roundtable-Client") != "browser":
                    return False
                cookie = SimpleCookie()
                try:
                    cookie.load(self.headers.get("Cookie", ""))
                    supplied = cookie.get(f"roundtable_session_{self.server.server_port}")
                    supplied = supplied.value if supplied else None
                except Exception:
                    return False
            return bool(supplied) and secrets.compare_digest(supplied, hub.token)

        def _send(self, code: int, body: bytes, ctype: str) -> None:
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("X-Content-Type-Options", "nosniff")
            if self.command == "POST" and urlparse(self.path).path == "/session" and code == 200:
                self.send_header("Set-Cookie", f"roundtable_session_{self.server.server_port}={hub.token}; Path=/; HttpOnly; SameSite=Strict")
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

            if url.path == "/session":
                self._json({"ok": True})
            elif url.path in ("/say", "/host"):
                text = body.get("text", "")
                if not isinstance(text, str):
                    self._json({"error": "Message must be text"}, 400)
                    return
                text = text.strip()
                if text:
                    hub.say(text)
                self._json({"ok": bool(text)})
            elif url.path == "/challenge":
                seq, quote = body.get("seq"), body.get("quote")
                question = body.get("question", "What evidence supports or weakens this claim?")
                if (type(seq) is not int or not isinstance(quote, str)
                        or not isinstance(question, str) or len(question) > 2000):
                    self._json({"error": "Choose a turn number, exact quote, and a short question"}, 400)
                    return
                try:
                    hub.challenge(seq, quote, question)
                except ValueError as exc:
                    self._json({"error": str(exc)}, 400)
                    return
                self._json({"ok": True})
            elif url.path == "/next":
                name = body.get("speaker")
                if name is not None and (not isinstance(name, str) or not hub.table.by_name(name)):
                    self._json({"error": "Unknown participant"}, 400)
                    return
                if not hub.start_one(name):
                    self._json({"error": "Pause and let the current speaker finish first"}, 409)
                    return
                self._json({"ok": True})
            elif url.path == "/auto":
                if not isinstance(body.get("on"), bool):
                    self._json({"error": "on must be true or false"}, 400)
                    return
                if not hub.set_auto(body["on"]):
                    self._json({"error": "A discussion is already running"}, 409)
                    return
                self._json({"ok": True, "running": hub.running, "auto": hub.running})
            elif url.path == "/round":
                independent = body.get("independent", False)
                if not isinstance(independent, bool):
                    self._json({"error": "independent must be true or false"}, 400)
                    return
                if not hub.run_round(independent=independent):
                    self._json({"error": "Pause and let the current speaker finish first"}, 409)
                    return
                self._json({"ok": True, "running": hub.running})
            elif url.path == "/new":
                topic = body.get("topic")
                if not isinstance(topic, str) or not topic.strip() or len(topic) > 2000:
                    self._json({"error": "Enter a topic between 1 and 2000 characters"}, 400)
                    return
                try:
                    changed = hub.new_topic(topic.strip())
                except ValueError as exc:
                    self._json({"error": str(exc)}, 400)
                    return
                if changed:
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

            q, snapshot = hub.subscribe_snapshot()
            try:
                self._emit(snapshot)
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
        hub._owe(0)
        hub.wake.set()
        httpd.shutdown()
        httpd.server_close()
        if hub.table.history:
            print(f"Transcript: {hub.table.export_markdown()}")
            print(f"Raw log:    {hub.table.transcript_path}")
    return 0
