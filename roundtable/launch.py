#!/usr/bin/env python3
"""Start or reopen the local Roundtable. Python 3.11+, no pip required."""
from pathlib import Path
import argparse
import json
import os
import subprocess
import sys
import time
import urllib.request
import urllib.parse
import webbrowser

BASE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--terminal", action="store_true")
    parser.add_argument("--no-open", action="store_true")
    parser.add_argument("--stop", action="store_true")
    parser.add_argument("topic", nargs="*")
    args = parser.parse_args()
    os.chdir(BASE)
    # Desktop launchers may have a smaller PATH than an interactive terminal.
    os.environ["PATH"] = os.pathsep.join([
        str(Path.home() / ".local/bin"), "/usr/lib/chatgpt/resources",
        "/usr/local/bin", os.environ.get("PATH", "")])
    if args.terminal:
        from roundtable.cli import main as cli_main
        return cli_main(["talk", *args.topic, "--transcripts", str(BASE / "transcripts")])
    runtime = BASE / ".runtime"
    runtime.mkdir(mode=0o700, exist_ok=True)
    state_file = runtime / "server.json"
    state = None
    try:
        candidate = json.loads(state_file.read_text())
        url = urllib.parse.urlsplit(candidate["url"])
        with urllib.request.urlopen(f"http://{url.netloc}/state?t={url.fragment}", timeout=2) as response:
            if json.load(response).get("type") == "snapshot":
                state = candidate
    except (OSError, ValueError, KeyError):
        pass
    if args.stop:
        if state:
            import signal
            os.kill(state["pid"], signal.SIGINT)
            print("Roundtable stopped. Transcripts are saved.")
        else:
            print("Roundtable is not running.")
        return 0
    if not state:
        state_file.unlink(missing_ok=True)
        env = dict(os.environ, ROUNDTABLE_STATE_PATH=str(state_file))
        topic = " ".join(args.topic) or "Your AI roundtable — choose New topic to begin"
        with (runtime / "server.log").open("w") as log:
            proc = subprocess.Popen([sys.executable, "-u", "-m", "roundtable", "web", topic,
                "--no-open", "--port", "0", "--transcripts", str(BASE / "transcripts")],
                env=env, cwd=BASE, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                start_new_session=True)
        for _ in range(150):
            if state_file.exists():
                try:
                    state = json.loads(state_file.read_text())
                    break
                except ValueError:
                    pass
            if proc.poll() is not None:
                break
            time.sleep(0.1)
        if not state:
            if proc.poll() is None:
                proc.terminate()
            print("Roundtable could not start. See .runtime/server.log.", file=sys.stderr)
            return 1
    print(state["url"])
    if not args.no_open:
        webbrowser.open(state["url"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
