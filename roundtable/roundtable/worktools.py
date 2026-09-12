"""Sandboxed execution primitives for goal-mode workers (see work.py).

Two independent, honestly-labeled layers -- never one overclaimed guarantee:

- Network isolation via `unshare --net`, verified against a real host in
  this environment (not assumed): a request that succeeds normally returns
  nothing at all -- not even a DNS lookup -- inside the namespace. Available
  wherever unprivileged user namespaces are permitted, which is the Ubuntu
  default.
- Filesystem isolation via bubblewrap (`bwrap`), used automatically when
  installed. Without it, there is no real filesystem boundary: `cwd` is set
  to the workspace and every file tool is confined to it, but a command
  given an absolute path still reaches the real filesystem, because
  chroot/pivot_root need privileges this process does not have. Every `run`
  result reports which level actually applied via `sandbox_level()` --
  reviewers and the host can see it, and it is never allowed to claim more
  than what happened.

The one boundary that IS fully enforced regardless of `run`'s sandbox
availability is path confinement for read_file/write_file/check_python: a
worker asking for "../../etc/cron.d/x" or an absolute path is refused before
touching the filesystem, checked here rather than trusted to the prompt.
"""

from __future__ import annotations

import ast
import difflib
import hashlib
import ipaddress
import json
import os
import shutil
import signal
import socket
import subprocess
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

MAX_FILE_BYTES = 2_000_000
MAX_FETCH_BYTES = 2_000_000
MAX_OUTPUT_BYTES = 200_000
RUN_TIMEOUT = 45.0
FETCH_TIMEOUT = 20.0

_BWRAP = shutil.which("bwrap")
_UNSHARE = shutil.which("unshare")


class SandboxError(ValueError):
    """A refusal the worker should see and adjust to -- never a crash."""


def copy_project(source: Path | None, workspace: Path) -> dict:
    """Copy a project into the workspace, or start empty.

    Never follows a symlink: a source directory containing one pointing at,
    say, ~/.ssh must not silently pull it into a workspace a model can then
    read. `.git` is skipped -- goal mode diffs the workspace itself; a
    cloned history and any credential-bearing remote or hook has no reason
    to be here.
    """
    if source is None:
        return {"source": None, "files": 0}
    count = 0
    for root, dirs, files in os.walk(source, followlinks=False):
        dirs[:] = [d for d in dirs if d != ".git"]
        rel_root = Path(root).relative_to(source)
        for name in files:
            src = Path(root) / name
            if src.is_symlink():
                continue
            dst = workspace / rel_root / name
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)
            count += 1
    return {"source": str(source), "files": count}


def _safe_path(workspace: Path, relative: object) -> Path:
    if not isinstance(relative, str) or not relative.strip():
        raise SandboxError("path must be a non-empty relative path")
    if relative.startswith("/") or relative.startswith("~"):
        raise SandboxError("path must be relative to the workspace, not absolute")
    resolved_root = workspace.resolve()
    candidate = (workspace / relative).resolve()
    try:
        candidate.relative_to(resolved_root)
    except ValueError:
        raise SandboxError(f"path escapes the workspace: {relative!r}") from None
    return candidate


def _is_public_host(hostname: str) -> bool:
    """Refuse a hostname resolving to a private, loopback, or link-local
    address -- the classic SSRF path (cloud metadata endpoints, localhost
    services, internal network probing) via a tool whose whole purpose is
    fetching wherever a worker points it.
    """
    try:
        infos = socket.getaddrinfo(hostname, None)
    except socket.gaierror:
        return False
    for _family, _type, _proto, _canon, sockaddr in infos:
        try:
            ip = ipaddress.ip_address(sockaddr[0])
        except ValueError:
            continue
        if (ip.is_private or ip.is_loopback or ip.is_link_local
                or ip.is_multicast or ip.is_reserved or ip.is_unspecified):
            return False
    return True


class WorkTools:
    def __init__(self, goal_dir: Path):
        self.goal_dir = Path(goal_dir)
        self.workspace = self.goal_dir / "workspace"
        self.baseline = self.goal_dir / "baseline"

    def path(self, relative: str) -> Path:
        return _safe_path(self.workspace, relative)

    def revision(self) -> str:
        """Content hash of the whole workspace.

        Lets a `finish` claim be checked against the code as it actually is
        right now, not the state a worker remembers or asserts -- a test run
        is only evidence of success if it ran after the last edit, not before.
        """
        digest = hashlib.sha256()
        for file in sorted(p for p in self.workspace.rglob("*") if p.is_file()):
            digest.update(str(file.relative_to(self.workspace)).encode())
            digest.update(file.read_bytes())
        return digest.hexdigest()[:16]

    def diff(self) -> str:
        return "".join(difflib.unified_diff(
            self._snapshot(self.baseline), self._snapshot(self.workspace),
            fromfile="baseline", tofile="workspace"))

    @staticmethod
    def _snapshot(root: Path) -> list[str]:
        lines: list[str] = []
        if not root.is_dir():
            return lines
        for file in sorted(p for p in root.rglob("*") if p.is_file()):
            rel = file.relative_to(root)
            try:
                text = file.read_text(errors="replace")
            except OSError:
                text = "<unreadable>"
            lines.append(f"--- {rel} ---\n")
            lines.extend(f"{line}\n" for line in text.splitlines())
        return lines

    def sandbox_level(self) -> str:
        if _BWRAP:
            return "filesystem+network isolated (bubblewrap)"
        if _UNSHARE:
            return ("network isolated only -- filesystem NOT contained "
                    "(install bubblewrap for full isolation)")
        return "unavailable -- command execution refused on this machine"

    # --- dispatch ------------------------------------------------------

    def execute(self, action: dict) -> dict:
        tool = action.get("tool")
        if tool == "list_files":
            return {"files": sorted(
                str(p.relative_to(self.workspace))
                for p in self.workspace.rglob("*") if p.is_file())}
        if tool == "read_file":
            return self._read_file(action.get("path"))
        if tool == "write_file":
            return self._write_file(action.get("path"), action.get("text"))
        if tool == "check_python":
            return self._check_python(action.get("paths"))
        if tool == "run":
            return self._run(action.get("argv"))
        if tool == "fetch_url":
            return self._fetch_url(action.get("url"))
        if tool == "search":
            return self._search(action.get("query"))
        raise SandboxError(f"unknown tool: {tool!r}")

    def _read_file(self, path: object) -> dict:
        target = self.path(path)
        if not target.is_file():
            raise SandboxError(f"no such file: {path!r}")
        if target.stat().st_size > MAX_FILE_BYTES:
            raise SandboxError(f"file exceeds the {MAX_FILE_BYTES} byte read limit")
        return {"path": path, "text": target.read_text(errors="replace")}

    def _write_file(self, path: object, text: object) -> dict:
        if not isinstance(text, str):
            raise SandboxError("write_file needs text")
        if len(text.encode()) > MAX_FILE_BYTES:
            raise SandboxError(f"write exceeds the {MAX_FILE_BYTES} byte limit")
        target = self.path(path)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
        return {"path": path, "bytes": len(text.encode())}

    def _check_python(self, paths: object) -> dict:
        """Pure syntax check -- ast.parse never executes the target's code,
        so this needs no sandbox of its own regardless of what `run` can do
        on this machine."""
        if not isinstance(paths, list) or not paths:
            raise SandboxError("check_python needs a non-empty list of paths")
        results = {}
        for rel in paths:
            target = self.path(rel)
            try:
                ast.parse(target.read_text(), filename=str(rel))
                results[rel] = {"ok": True}
            except SyntaxError as exc:
                results[rel] = {"ok": False,
                                "error": f"{exc.msg} (line {exc.lineno})"}
            except OSError as exc:
                results[rel] = {"ok": False, "error": str(exc)}
        return {"ok": all(r["ok"] for r in results.values()), "results": results}

    # --- run: the one tool that needs a real sandbox --------------------

    def _run(self, argv: object) -> dict:
        if (not isinstance(argv, list) or not argv
                or not all(isinstance(a, str) for a in argv)):
            raise SandboxError("run needs a non-empty list of string arguments")
        if not _UNSHARE and not _BWRAP:
            raise SandboxError(
                "command execution is unavailable on this machine: neither "
                "bubblewrap nor unshare is installed, and running "
                "unsandboxed is refused. Use needs_input to report this "
                "rather than treating a syntax check as a passing test.")

        env = {"PATH": "/usr/bin:/bin:/usr/local/bin", "HOME": str(self.workspace),
               "TMPDIR": str(self.workspace), "LANG": "C.UTF-8"}

        # Resource caps via the shell's own ulimit, applied inside the
        # sandbox before exec -- not Python's preexec_fn, which the
        # standard library explicitly documents as unsafe to combine with
        # fork() from a multi-threaded process, and this manager always
        # runs its work loop on a background thread.
        # dash (Ubuntu's /bin/sh) has no -u option at all -- not a
        # bwrap or permissions issue, confirmed by isolating the exec
        # chain outside the sandbox first. prlimit is a real binary,
        # not a shell builtin, and covers it; fall back to a plain
        # exec (still CPU/memory/file-size capped) if it is missing.
        capped = ("ulimit -t 45 || echo ulimit-t-failed >&2; "
                  "ulimit -v 2097152 || echo ulimit-v-failed >&2; "
                  "ulimit -f 4000000 || echo ulimit-f-failed >&2; "
                  "command -v prlimit >/dev/null "
                  '&& exec prlimit --nproc=64 -- "$@"; '
                  'exec "$@"')
        wrapped = ["/bin/sh", "-c", capped, "sh", *argv]

        if _BWRAP:
            command = [_BWRAP,
                      "--ro-bind", "/usr", "/usr", "--ro-bind", "/bin", "/bin",
                      "--ro-bind-try", "/lib", "/lib",
                      "--ro-bind-try", "/lib64", "/lib64",
                      "--ro-bind-try", "/etc/alternatives", "/etc/alternatives",
                      "--dev", "/dev", "--proc", "/proc",
                      "--bind", str(self.workspace), str(self.workspace),
                      "--chdir", str(self.workspace),
                      "--unshare-all", "--die-with-parent", "--",
                      *wrapped]
        else:
            command = [_UNSHARE, "--net", "--", *wrapped]

        return self._spawn(command, env)

    def _spawn(self, command: list[str], env: dict) -> dict:
        """Bounded subprocess lifetime with real process-group cleanup.

        Mirrors local.py's stream_cli exactly: start_new_session=True plus
        os.killpg on timeout via a Timer -- not subprocess.run's own
        timeout, which only reaches the direct child and can leave
        grandchildren (a test runner's own workers, say) still running.
        """
        started = time.monotonic()
        timed_out = threading.Event()
        try:
            proc = subprocess.Popen(
                command, cwd=self.workspace, env=env,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                start_new_session=True)
        except OSError as exc:
            raise SandboxError(f"could not start the sandbox: {exc}") from None

        def expire():
            timed_out.set()
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

        timer = threading.Timer(RUN_TIMEOUT, expire)
        timer.start()
        try:
            stdout, stderr = proc.communicate()
        finally:
            timer.cancel()

        return {
            "ok": (not timed_out.is_set()) and proc.returncode == 0,
            "returncode": proc.returncode,
            "timed_out": timed_out.is_set(),
            "stdout": stdout.decode(errors="replace")[-MAX_OUTPUT_BYTES:],
            "stderr": (stderr.decode(errors="replace")
                      + ("\n[killed after 45s]" if timed_out.is_set() else "")
                      )[-MAX_OUTPUT_BYTES:],
            "seconds": round(time.monotonic() - started, 2),
            "sandbox": self.sandbox_level(),
        }

    # --- network tools ---------------------------------------------------

    def _fetch_url(self, url: object) -> dict:
        if not isinstance(url, str) or not url.strip():
            raise SandboxError("fetch_url needs a url")
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme not in ("http", "https"):
            raise SandboxError("only http:// and https:// URLs may be fetched")
        if not parsed.hostname:
            raise SandboxError("url has no host")
        if not _is_public_host(parsed.hostname):
            raise SandboxError(
                f"refusing to fetch {parsed.hostname!r}: it resolves to a "
                "private, loopback, or link-local address")
        request = urllib.request.Request(
            url, headers={"User-Agent": "Roundtable-goal-worker/1 (+local)"})
        try:
            with urllib.request.urlopen(request, timeout=FETCH_TIMEOUT) as response:
                body = response.read(MAX_FETCH_BYTES + 1)
                content_type = response.headers.get("Content-Type", "")
                status = response.status
        except urllib.error.URLError as exc:
            raise SandboxError(f"fetch failed: {exc}") from None
        truncated = len(body) > MAX_FETCH_BYTES
        if truncated:
            body = body[:MAX_FETCH_BYTES]
        return {"url": url, "status": status, "content_type": content_type,
                "truncated": truncated, "text": body.decode(errors="replace")}

    def _search(self, query: object) -> dict:
        """Requires a real search API key. Never falls back to scraping a
        search engine's own results page -- that is a ToS violation this
        project's own operating rules explicitly rule out, not merely a
        quality shortcut.
        """
        key = os.environ.get("BRAVE_SEARCH_API_KEY")
        if not key:
            raise SandboxError(
                "web search is not configured: export BRAVE_SEARCH_API_KEY "
                "(free tier at api.search.brave.com) or use fetch_url on a "
                "specific page you already have the address for.")
        if not isinstance(query, str) or not query.strip():
            raise SandboxError("search needs a query")
        url = ("https://api.search.brave.com/res/v1/web/search?"
               + urllib.parse.urlencode({"q": query.strip()[:400]}))
        request = urllib.request.Request(
            url, headers={"X-Subscription-Token": key, "Accept": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=15) as response:
                data = json.loads(response.read())
        except urllib.error.URLError as exc:
            raise SandboxError(f"search failed: {exc}") from None
        results = [
            {"title": r.get("title"), "url": r.get("url"),
             "snippet": r.get("description")}
            for r in (data.get("web", {}).get("results") or [])[:8]
        ]
        return {"query": query, "results": results}
