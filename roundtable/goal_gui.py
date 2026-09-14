#!/usr/bin/env python3
"""goal_gui.py -- a native Linux desktop app for Roundtable's goal mode.

Discussion mode already has a working GUI (the browser app launched by
roundtable.sh). Goal mode has only ever had a terminal driver (goal.py) --
this fills that gap with an actual window: create/watch/resume goals,
read the live step-by-step log, and see the patch once one is ready.

    ./goal-gui.sh
    python3 goal_gui.py

Uses the same WorkManager as goal.py and the browser app's discussion
engine -- no new backend, just a Tkinter front end over what already
exists (see CHARTER.md rule 12: check before you build).
"""
from __future__ import annotations

import json
import queue
import sys
import threading
import tkinter as tk
from pathlib import Path
from tkinter import filedialog, messagebox, simpledialog, ttk

sys.path.insert(0, str(Path(__file__).parent))

from roundtable import config
from roundtable.work import WorkManager

ROOT = Path.home() / ".roundtable-goals"
MODES = ["coding", "research", "ideas", "money"]
POLL_MS = 200


def short(identifier: str) -> str:
    return identifier[:10]


class NewGoalDialog(tk.Toplevel):
    def __init__(self, parent, on_create):
        super().__init__(parent)
        self.title("New goal")
        self.resizable(False, False)
        self.transient(parent)
        self.on_create = on_create

        pad = {"padx": 8, "pady": 4}
        ttk.Label(self, text="Task").grid(row=0, column=0, sticky="ne", **pad)
        self.task = tk.Text(self, width=52, height=4, wrap="word")
        self.task.grid(row=0, column=1, columnspan=2, **pad)

        ttk.Label(self, text="Mode").grid(row=1, column=0, sticky="e", **pad)
        self.mode = ttk.Combobox(self, values=MODES, state="readonly", width=12)
        self.mode.set(MODES[0])
        self.mode.grid(row=1, column=1, sticky="w", **pad)

        ttk.Label(self, text="Project folder").grid(row=2, column=0, sticky="e", **pad)
        self.project = tk.Entry(self, width=40)
        self.project.grid(row=2, column=1, **pad)
        ttk.Button(self, text="Browse…", command=self._browse).grid(row=2, column=2, **pad)

        ttk.Label(self, text="Step budget").grid(row=3, column=0, sticky="e", **pad)
        self.steps = tk.Spinbox(self, from_=2, to=100, width=6)
        self.steps.delete(0, "end")
        self.steps.insert(0, "20")
        self.steps.grid(row=3, column=1, sticky="w", **pad)

        self.start_now = tk.BooleanVar(value=True)
        ttk.Checkbutton(self, text="Start immediately", variable=self.start_now).grid(
            row=4, column=1, sticky="w", **pad)

        buttons = ttk.Frame(self)
        buttons.grid(row=5, column=0, columnspan=3, pady=(4, 8))
        ttk.Button(buttons, text="Cancel", command=self.destroy).pack(side="left", padx=6)
        ttk.Button(buttons, text="Create", command=self._submit).pack(side="left", padx=6)

        self.task.focus_set()
        self.grab_set()

    def _browse(self):
        path = filedialog.askdirectory(parent=self, mustexist=True)
        if path:
            self.project.delete(0, "end")
            self.project.insert(0, path)

    def _submit(self):
        task = self.task.get("1.0", "end").strip()
        if not task:
            messagebox.showerror("New goal", "Enter a task.", parent=self)
            return
        try:
            steps = int(self.steps.get())
        except ValueError:
            messagebox.showerror("New goal", "Step budget must be a number.", parent=self)
            return
        try:
            self.on_create(task, self.mode.get(), self.project.get().strip(),
                            steps, self.start_now.get())
        except ValueError as exc:
            messagebox.showerror("New goal", str(exc), parent=self)
            return
        self.destroy()


class ResumeDialog(tk.Toplevel):
    def __init__(self, parent, on_resume):
        super().__init__(parent)
        self.title("Resume goal")
        self.resizable(False, False)
        self.transient(parent)
        self.on_resume = on_resume

        pad = {"padx": 8, "pady": 4}
        ttk.Label(self, text="Follow-up feedback (optional)").grid(
            row=0, column=0, columnspan=2, sticky="w", **pad)
        self.feedback = tk.Text(self, width=52, height=4, wrap="word")
        self.feedback.grid(row=1, column=0, columnspan=2, **pad)

        ttk.Label(self, text="Additional steps").grid(row=2, column=0, sticky="e", **pad)
        self.steps = tk.Spinbox(self, from_=0, to=100, width=6)
        self.steps.delete(0, "end")
        self.steps.insert(0, "8")
        self.steps.grid(row=2, column=1, sticky="w", **pad)

        buttons = ttk.Frame(self)
        buttons.grid(row=3, column=0, columnspan=2, pady=(4, 8))
        ttk.Button(buttons, text="Cancel", command=self.destroy).pack(side="left", padx=6)
        ttk.Button(buttons, text="Resume", command=self._submit).pack(side="left", padx=6)
        self.grab_set()

    def _submit(self):
        try:
            extra = int(self.steps.get())
        except ValueError:
            messagebox.showerror("Resume", "Additional steps must be a number.", parent=self)
            return
        try:
            self.on_resume(self.feedback.get("1.0", "end").strip(), extra)
        except ValueError as exc:
            messagebox.showerror("Resume", str(exc), parent=self)
            return
        self.destroy()


class TextViewer(tk.Toplevel):
    def __init__(self, parent, title, content):
        super().__init__(parent)
        self.title(title)
        self.geometry("720x500")
        text = tk.Text(self, wrap="word")
        scroll = ttk.Scrollbar(self, command=text.yview)
        text.configure(yscrollcommand=scroll.set)
        text.pack(side="left", fill="both", expand=True)
        scroll.pack(side="right", fill="y")
        text.insert("1.0", content)
        text.configure(state="disabled")


class GoalGUI(tk.Tk):
    def __init__(self):
        super().__init__()
        self.title("Roundtable — Goal Mode")
        self.geometry("1040x660")
        self.events: "queue.Queue[dict]" = queue.Queue()
        self.main_thread_calls: "queue.Queue" = queue.Queue()
        self.row_ids: list[str] = []  # treeview iid == goal id, kept for order
        self.seats_error = ""
        try:
            seats, _ = config.resolve(include_cli=True)
        except Exception as exc:  # pragma: no cover -- surfaced in the UI instead
            seats, self.seats_error = [], str(exc)
        self.manager = WorkManager(ROOT, seats, notify=self._on_notify)

        self._build_widgets()
        self._refresh_seats_label()
        self._apply_state(self.manager.snapshot())
        self.after(POLL_MS, self._pump_events)
        self.protocol("WM_DELETE_WINDOW", self._on_close)

    # ------------------------------------------------------------------
    # layout

    def _build_widgets(self):
        top = ttk.Frame(self, padding=6)
        top.pack(side="top", fill="x")
        self.seats_label = ttk.Label(top, text="Discovering connections…")
        self.seats_label.pack(side="left")
        ttk.Button(top, text="Rescan connections", command=self._rescan).pack(side="right")

        body = ttk.Panedwindow(self, orient="horizontal")
        body.pack(side="top", fill="both", expand=True, padx=6, pady=(0, 6))

        left = ttk.Frame(body)
        body.add(left, weight=1)
        columns = ("mode", "status")
        self.tree = ttk.Treeview(left, columns=columns, show="tree headings", height=20)
        self.tree.heading("#0", text="Goal")
        self.tree.heading("mode", text="Mode")
        self.tree.heading("status", text="Status")
        self.tree.column("#0", width=220)
        self.tree.column("mode", width=80, anchor="center")
        self.tree.column("status", width=100, anchor="center")
        self.tree.pack(side="top", fill="both", expand=True)
        self.tree.bind("<<TreeviewSelect>>", lambda _e: self._show_selected())
        ttk.Button(left, text="New goal…", command=self._new_goal).pack(
            side="top", fill="x", pady=(6, 0))

        right = ttk.Frame(body)
        body.add(right, weight=2)

        self.task_label = ttk.Label(right, text="Select a goal, or create one.",
                                     wraplength=640, font=("", 11, "bold"))
        self.task_label.pack(side="top", anchor="w")
        self.status_label = ttk.Label(right, text="")
        self.status_label.pack(side="top", anchor="w", pady=(2, 6))

        buttons = ttk.Frame(right)
        buttons.pack(side="top", fill="x", pady=(0, 6))
        self.resume_btn = ttk.Button(buttons, text="Resume…", command=self._resume,
                                      state="disabled")
        self.resume_btn.pack(side="left", padx=(0, 4))
        self.pause_btn = ttk.Button(buttons, text="Pause", command=self._pause,
                                     state="disabled")
        self.pause_btn.pack(side="left", padx=4)
        self.patch_btn = ttk.Button(buttons, text="View patch", command=self._view_patch,
                                     state="disabled")
        self.patch_btn.pack(side="left", padx=4)
        self.json_btn = ttk.Button(buttons, text="Full detail (JSON)",
                                    command=self._view_json, state="disabled")
        self.json_btn.pack(side="left", padx=4)
        self.outcome_btn = ttk.Button(buttons, text="Record outcome…",
                                       command=self._record_outcome, state="disabled")
        self.outcome_btn.pack(side="left", padx=4)

        ttk.Label(right, text="Activity log").pack(side="top", anchor="w")
        log_frame = ttk.Frame(right)
        log_frame.pack(side="top", fill="both", expand=True)
        self.log = tk.Text(log_frame, wrap="word", state="disabled", height=20)
        log_scroll = ttk.Scrollbar(log_frame, command=self.log.yview)
        self.log.configure(yscrollcommand=log_scroll.set)
        self.log.pack(side="left", fill="both", expand=True)
        log_scroll.pack(side="right", fill="y")

    # ------------------------------------------------------------------
    # WorkManager plumbing -- notify() fires on the manager's own worker
    # thread, and _run_off_thread's workers fire on their own throwaway
    # threads, so neither may touch a widget or call .after() itself --
    # Tk raises "main thread is not in main loop" if a background thread
    # calls .after() directly (confirmed by a scripted test, not assumed).
    # Both only ever put data/callables on a queue; only _pump_events,
    # which the main thread keeps re-scheduling via .after(), drains them.

    def _on_notify(self, state):
        self.events.put(state)

    def _pump_events(self):
        state = None
        try:
            while True:
                state = self.events.get_nowait()
        except queue.Empty:
            pass
        if state is not None:
            self._apply_state(state)
        try:
            while True:
                self.main_thread_calls.get_nowait()()
        except queue.Empty:
            pass
        self.after(POLL_MS, self._pump_events)

    def _apply_state(self, state):
        self._state = state
        selected = self._selected_id()
        existing = set(self.tree.get_children(""))
        seen = set()
        for goal in state["goals"]:
            iid = goal["id"]
            seen.add(iid)
            label = f'{short(iid)}  {goal["task"][:40]}'
            values = (goal["mode"], goal["status"])
            if iid in existing:
                self.tree.item(iid, text=label, values=values)
            else:
                self.tree.insert("", "end", iid=iid, text=label, values=values)
        for stale in existing - seen:
            self.tree.delete(stale)
        if selected and selected in seen:
            self.tree.selection_set(selected)
        self._show_selected()

    def _selected_id(self):
        sel = self.tree.selection()
        return sel[0] if sel else None

    def _current_goal(self):
        identifier = self._selected_id()
        if not identifier or not hasattr(self, "_state"):
            return None
        return next((g for g in self._state["goals"] if g["id"] == identifier), None)

    # ------------------------------------------------------------------
    # detail pane

    def _show_selected(self):
        goal = self._current_goal()
        for widget in (self.resume_btn, self.pause_btn, self.patch_btn,
                       self.json_btn, self.outcome_btn):
            widget.configure(state="disabled")
        if not goal:
            self.task_label.configure(text="Select a goal, or create one.")
            self.status_label.configure(text="")
            self._set_log("")
            return
        self.task_label.configure(text=f'[{goal["mode"]}] {goal["task"]}')
        busy_note = " (running)" if self._state.get("current") == goal["id"] else ""
        self.status_label.configure(
            text=f'Status: {goal["status"]}{busy_note} — {goal["message"]}  '
                 f'[{goal["steps"]}/{goal["limit"]} steps]')
        self.json_btn.configure(state="normal")
        if not self._state.get("busy"):
            if goal["status"] in ("paused", "needs_input", "budget", "ready"):
                self.resume_btn.configure(state="normal")
        elif self._state.get("current") == goal["id"]:
            self.pause_btn.configure(state="normal")
        if goal["status"] == "ready":
            self.patch_btn.configure(state="normal")
            self.outcome_btn.configure(state="normal")
        lines = []
        if goal["plan"]:
            lines.append("Plan:")
            lines += [f"  {i + 1}. {step}" for i, step in enumerate(goal["plan"])]
            lines.append("")
        lines.append("Events:")
        for event in goal["events"]:
            lines.append(f'  #{event["id"]} [{event["kind"]}] {event["summary"][:300]}')
        self._set_log("\n".join(lines))

    def _set_log(self, text):
        self.log.configure(state="normal")
        at_bottom = self.log.yview()[1] >= 0.999
        self.log.delete("1.0", "end")
        self.log.insert("1.0", text)
        self.log.configure(state="disabled")
        if at_bottom:
            self.log.see("end")

    # ------------------------------------------------------------------
    # actions

    def _refresh_seats_label(self):
        names = [p.name for p in self.manager.seats()]
        if self.seats_error:
            self.seats_label.configure(text=f"Connection discovery failed: {self.seats_error}")
        elif names:
            self.seats_label.configure(text="Connected: " + ", ".join(names))
        else:
            self.seats_label.configure(
                text="No CLI or local-model seats found — sign in to a CLI or "
                     "start Ollama, then Rescan.")

    def _rescan(self):
        try:
            seats, _ = config.resolve(include_cli=True)
            self.seats_error = ""
        except Exception as exc:
            seats, self.seats_error = [], str(exc)
        self.manager.participants = seats
        self._refresh_seats_label()

    def _run_off_thread(self, work, on_done=None):
        """Run a WorkManager call in the background and hop back to the Tk
        main thread for the result. manager.create() copies the whole
        target project synchronously (a real, possibly slow filesystem
        walk) -- calling it directly from a dialog's button handler freezes
        the entire window for that long, since Tkinter has one thread.
        manager.start() is normally fast, but nothing here should ever
        assume that and risk it again."""
        def worker():
            try:
                result = work()
            except ValueError as exc:
                # Python deletes the name bound by "except ... as exc" the
                # moment this block ends, so a lambda capturing `exc` itself
                # sees a dead reference once it actually runs later on the
                # main thread. Capture the message now, while it's alive.
                message = str(exc)
                self.main_thread_calls.put(lambda: messagebox.showerror("Roundtable", message))
                return
            self.main_thread_calls.put(lambda: self._after_manager_call(result, on_done))
        threading.Thread(target=worker, daemon=True).start()

    def _after_manager_call(self, result, on_done):
        self._apply_state(self.manager.snapshot())
        if on_done:
            on_done(result)

    def _new_goal(self):
        def create(task, mode, project, steps, start_now):
            def work():
                identifier = self.manager.create(task, mode=mode, project=project,
                                                  max_steps=steps)
                if start_now:
                    self.manager.start(identifier)
                return identifier

            def select(identifier):
                if identifier in self.tree.get_children(""):
                    self.tree.selection_set(identifier)
            self._run_off_thread(work, select)
        NewGoalDialog(self, create)

    def _resume(self):
        goal = self._current_goal()
        if not goal:
            return

        def resume(feedback, extra_steps):
            self._run_off_thread(
                lambda: self.manager.start(goal["id"], feedback=feedback,
                                           extra_steps=extra_steps))
        ResumeDialog(self, resume)

    def _pause(self):
        self.manager.pause()

    def _view_patch(self):
        goal = self._current_goal()
        if not goal:
            return
        patch_path = self.manager.root / goal["id"] / "changes.patch"
        content = patch_path.read_text() if patch_path.is_file() else "(no patch written)"
        TextViewer(self, f"Patch — {short(goal['id'])}", content or "(no changes)")

    def _view_json(self):
        goal = self._current_goal()
        if not goal:
            return
        full = self.manager.get(goal["id"])
        TextViewer(self, f"Detail — {short(goal['id'])}",
                   json.dumps(full, indent=2, ensure_ascii=False))

    def _record_outcome(self):
        goal = self._current_goal()
        if not goal:
            return
        note = simpledialog.askstring(
            "Record outcome", "What actually happened (payment, time spent, result)?",
            parent=self)
        if note and note.strip():
            try:
                self.manager.outcome(goal["id"], {"note": note.strip()})
            except ValueError as exc:
                messagebox.showerror("Record outcome", str(exc), parent=self)

    def _on_close(self):
        if self.manager.busy:
            if not messagebox.askyesno(
                    "Quit", "A goal is still running. Pause it and quit?"):
                return
            self.manager.pause()
        self.destroy()


def main() -> None:
    app = GoalGUI()
    app.mainloop()


if __name__ == "__main__":
    main()
