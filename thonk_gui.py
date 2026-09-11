#!/usr/bin/env python3
"""
thonk_gui.py - a window for the thonk granular engine.

Laid out the way thOnk_0+2 worked: pick a score, read what it says about
itself, hit Thonk, choose an input file, choose where the output goes, then
watch the console fill up while it renders. Stop whenever you have had enough;
the output file is complete and playable at every moment.

    python3 thonk_gui.py

Needs numpy and tkinter. Tkinter ships with the python.org installers and with
Homebrew's python-tk; Apple's /usr/bin/python3 has it too.

tkinter is imported inside run_gui() rather than at module scope, so the
Worker class below can be exercised without a display:

    python3 thonk_gui.py --selftest
"""

import os
import queue
import sys
import threading
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)

from thonk import SCORES, Session, fix_header, hms  # noqa: E402


# --------------------------------------------------------------------------
# the worker - no GUI toolkit in here
# --------------------------------------------------------------------------


class Worker:
    """Runs a Session on a thread and posts events to a queue.

    Events are ("progress", Progress), ("log", str), ("done", dict) or
    ("error", str). Nothing here touches the GUI; the window polls drain().
    """

    def __init__(self, settings):
        self.settings = dict(settings)
        self.events = queue.Queue()
        self.session = None
        self.thread = None

    def start(self):
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def stop(self):
        if self.session is not None:
            self.session.stop_requested = True

    @property
    def running(self):
        return self.thread is not None and self.thread.is_alive()

    def drain(self):
        """Return every event queued since the last call."""
        out = []
        while True:
            try:
                out.append(self.events.get_nowait())
            except queue.Empty:
                return out

    def _run(self):
        s = self.settings
        try:
            session = Session.from_file(
                s["input"], s["output"], score=s["score"], duration=s["duration"],
                gain=s["gain"], overflow=s["overflow"], autogain=s["autogain"],
                seed=s.get("seed"), block=s.get("block", 0.5))
            self.session = session
            self.events.put(("log", "input    %s" % os.path.basename(s["input"])))
            self.events.put(("log", "         %.2f s mono, %d Hz, peak %.2f"
                             % (len(session.source) / session.in_rate,
                                session.in_rate, session.peak)))
            self.events.put(("log", "score    %s - %s"
                             % (s["score"], SCORES[s["score"]]["description"])))
            self.events.put(("log", "output   %s stereo, %d Hz, seed %d"
                             % (hms(session.duration), session.rate, session.seed)))
            self.events.put(("log", ""))
            for progress in session.run():
                self.events.put(("progress", progress))
            self.events.put(("done", {
                "seconds": session.written_seconds,
                "grains": session.engine.grains,
                "elapsed": session.elapsed,
                "overflows": session.overflows,
                "stopped": session.stop_requested,
                "output": s["output"],
                "overflow": s["overflow"],
            }))
        except Exception as exc:  # surfaced in the window, not the terminal
            self.events.put(("error", "%s: %s" % (type(exc).__name__, exc)))
            self.events.put(("log", traceback.format_exc()))


# --------------------------------------------------------------------------
# the window
# --------------------------------------------------------------------------


def run_gui():
    import tkinter as tk
    from tkinter import filedialog, messagebox, ttk

    AUDIO_TYPES = [("Sound files", "*.aiff *.aif *.aifc *.wav"), ("All files", "*")]

    class ThonkWindow:
        POLL_MS = 120

        def __init__(self, root):
            self.root = root
            self.worker = None
            self.last_output = None
            root.title("thOnk")
            root.minsize(620, 520)
            self._set_icon()

            outer = ttk.Frame(root, padding=14)
            outer.pack(fill="both", expand=True)
            outer.columnconfigure(0, weight=1)

            self.in_var = tk.StringVar()
            self.out_var = tk.StringVar()
            self.score_var = tk.StringVar(value="flowing")
            self.minutes_var = tk.StringVar(value="20")
            self.gain_var = tk.DoubleVar(value=1.0)
            self.autogain_var = tk.BooleanVar(value=True)
            self.wrap_var = tk.BooleanVar(value=True)
            self.seed_var = tk.StringVar()

            self._build_files(outer)
            self._build_score(outer)
            self._build_options(outer)
            self._build_console(outer)
            self._build_buttons(outer)
            self._build_menu()

            self.on_score_change()
            self.log("thOnk - granular synthesis after thOnk_0+2 "
                     "(Arjen van der Schoot / Audio Ease, 1996-1999).")
            self.log("Pick a score and hit Thonk. A short, transient-rich mono "
                     "file works best.")
            self.log("")
            root.protocol("WM_DELETE_WINDOW", self.on_quit)

        # ---------------------------------------------------------- layout

        def _set_icon(self):
            for name in ("thOnk_icon.png", "thOnk.png"):
                path = os.path.join(HERE, name)
                if os.path.exists(path):
                    try:
                        self.icon = tk.PhotoImage(file=path)
                        self.root.iconphoto(True, self.icon)
                    except Exception:
                        pass
                    return

        def _build_files(self, parent):
            frame = ttk.LabelFrame(parent, text="Files", padding=10)
            frame.grid(row=0, column=0, sticky="ew")
            frame.columnconfigure(1, weight=1)
            ttk.Label(frame, text="In").grid(row=0, column=0, sticky="w")
            ttk.Entry(frame, textvariable=self.in_var).grid(
                row=0, column=1, sticky="ew", padx=6)
            ttk.Button(frame, text="Choose...", command=self.pick_input).grid(
                row=0, column=2)
            ttk.Label(frame, text="Out").grid(row=1, column=0, sticky="w", pady=(6, 0))
            ttk.Entry(frame, textvariable=self.out_var).grid(
                row=1, column=1, sticky="ew", padx=6, pady=(6, 0))
            ttk.Button(frame, text="Choose...", command=self.pick_output).grid(
                row=1, column=2, pady=(6, 0))

        def _build_score(self, parent):
            frame = ttk.LabelFrame(parent, text="Score", padding=10)
            frame.grid(row=1, column=0, sticky="ew", pady=(12, 0))
            frame.columnconfigure(1, weight=1)
            names = list(SCORES)
            self.score_menu = ttk.OptionMenu(frame, self.score_var, names[0], *names,
                                             command=lambda _v: self.on_score_change())
            self.score_menu.grid(row=0, column=0, sticky="w")
            self.score_note = ttk.Label(frame, text="", wraplength=430,
                                        justify="left", foreground="#555555")
            self.score_note.grid(row=0, column=1, sticky="w", padx=10)

        def _build_options(self, parent):
            frame = ttk.LabelFrame(parent, text="Output", padding=10)
            frame.grid(row=2, column=0, sticky="ew", pady=(12, 0))
            ttk.Label(frame, text="Minutes").grid(row=0, column=0, sticky="w")
            # a plain Entry rather than ttk.Spinbox: Spinbox needs Tk 8.6, and
            # Apple's system Python still links Tk 8.5
            ttk.Entry(frame, textvariable=self.minutes_var, width=7).grid(
                row=0, column=1, sticky="w", padx=(6, 18))
            ttk.Label(frame, text="Level").grid(row=0, column=2, sticky="w")
            ttk.Scale(frame, from_=0.05, to=1.0, variable=self.gain_var,
                      orient="horizontal", length=110).grid(row=0, column=3,
                                                            sticky="w", padx=(6, 18))
            ttk.Label(frame, text="Seed").grid(row=0, column=4, sticky="w")
            ttk.Entry(frame, textvariable=self.seed_var, width=10).grid(
                row=0, column=5, sticky="w", padx=6)
            ttk.Checkbutton(frame, text="Keep headroom automatically",
                            variable=self.autogain_var).grid(
                row=1, column=0, columnspan=3, sticky="w", pady=(8, 0))
            ttk.Checkbutton(frame, text="Wrap past full scale, as the original did",
                            variable=self.wrap_var).grid(
                row=1, column=3, columnspan=3, sticky="w", pady=(8, 0))

        def _build_console(self, parent):
            frame = ttk.LabelFrame(parent, text="Console", padding=10)
            frame.grid(row=3, column=0, sticky="nsew", pady=(12, 0))
            parent.rowconfigure(3, weight=1)
            frame.columnconfigure(0, weight=1)
            frame.rowconfigure(1, weight=1)

            self.status = ttk.Label(frame, text="Idle", font=("Menlo", 11))
            self.status.grid(row=0, column=0, sticky="w")
            self.text = tk.Text(frame, height=11, wrap="word", relief="flat",
                                font=("Menlo", 10), background="#f4f4f2",
                                highlightthickness=0)
            self.text.grid(row=1, column=0, sticky="nsew", pady=(6, 6))
            self.text.configure(state="disabled")
            bar = ttk.Scrollbar(frame, command=self.text.yview)
            bar.grid(row=1, column=1, sticky="ns", pady=(6, 6))
            self.text.configure(yscrollcommand=bar.set)
            self.progress = ttk.Progressbar(frame, mode="determinate", maximum=1.0)
            self.progress.grid(row=2, column=0, columnspan=2, sticky="ew")

        def _build_buttons(self, parent):
            frame = ttk.Frame(parent)
            frame.grid(row=4, column=0, sticky="ew", pady=(12, 0))
            self.thonk_btn = ttk.Button(frame, text="Thonk", command=self.on_thonk)
            self.thonk_btn.pack(side="left")
            self.reveal_btn = ttk.Button(frame, text="Show file",
                                         command=self.on_reveal, state="disabled")
            self.reveal_btn.pack(side="left", padx=(8, 0))
            self.play_btn = ttk.Button(frame, text="Play", command=self.on_play,
                                       state="disabled")
            self.play_btn.pack(side="left", padx=(8, 0))
            ttk.Button(frame, text="Quit", command=self.on_quit).pack(side="right")

        def _build_menu(self):
            menubar = tk.Menu(self.root)
            filemenu = tk.Menu(menubar, tearoff=0)
            filemenu.add_command(label="Choose input...", command=self.pick_input)
            filemenu.add_command(label="Choose output...", command=self.pick_output)
            filemenu.add_separator()
            filemenu.add_command(label="Repair a truncated file...", command=self.on_fix)
            menubar.add_cascade(label="File", menu=filemenu)
            helpmenu = tk.Menu(menubar, tearoff=0)
            helpmenu.add_command(label="About thOnk", command=self.on_about)
            menubar.add_cascade(label="Help", menu=helpmenu)
            self.root.configure(menu=menubar)

        # ------------------------------------------------------- behaviour

        def log(self, line):
            self.text.configure(state="normal")
            self.text.insert("end", line + "\n")
            self.text.see("end")
            self.text.configure(state="disabled")

        def on_score_change(self):
            score = SCORES[self.score_var.get()]
            self.score_note.configure(text=score["description"])
            if not self.worker or not self.worker.running:
                self.minutes_var.set("%g" % (score["duration"] / 60.0))

        def pick_input(self):
            path = filedialog.askopenfilename(title="Input sound file",
                                              filetypes=AUDIO_TYPES)
            if path:
                self.in_var.set(path)
                if not self.out_var.get():
                    stem = os.path.splitext(path)[0]
                    self.out_var.set(stem + "_thonk.aiff")
            return path

        def pick_output(self):
            path = filedialog.asksaveasfilename(
                title="Save output as", defaultextension=".aiff",
                filetypes=[("AIFF", "*.aiff"), ("WAV", "*.wav")])
            if path:
                self.out_var.set(path)
            return path

        def on_thonk(self):
            if self.worker and self.worker.running:
                self.worker.stop()
                self.thonk_btn.configure(text="Stopping...", state="disabled")
                return
            # the original asked for the files at this point, so do the same
            # when they have not been filled in yet
            if not self.in_var.get() and not self.pick_input():
                return
            if not self.out_var.get() and not self.pick_output():
                return
            try:
                minutes = float(self.minutes_var.get())
                if minutes <= 0:
                    raise ValueError("length must be greater than zero")
                seed_text = self.seed_var.get().strip()
                seed = int(seed_text) if seed_text else None
            except ValueError as exc:
                messagebox.showerror("thOnk", str(exc))
                return

            settings = {
                "input": self.in_var.get(),
                "output": self.out_var.get(),
                "score": self.score_var.get(),
                "duration": minutes * 60.0,
                "gain": float(self.gain_var.get()),
                "overflow": "wrap" if self.wrap_var.get() else "clip",
                "autogain": bool(self.autogain_var.get()),
                "seed": seed,
                "block": 0.5,
            }
            self.worker = Worker(settings)
            self.worker.start()
            self.thonk_btn.configure(text="Stop")
            self.reveal_btn.configure(state="disabled")
            self.play_btn.configure(state="disabled")
            self.progress.configure(value=0.0)
            self.root.after(self.POLL_MS, self.poll)

        def poll(self):
            if not self.worker:
                return
            for kind, payload in self.worker.drain():
                if kind == "progress":
                    self.progress.configure(value=payload.fraction)
                    self.status.configure(
                        text="%s / %s   %5.0f grains/sec   %d grains   %.1fx realtime"
                        % (hms(payload.t), hms(payload.duration), payload.density,
                           payload.grains, payload.speed))
                elif kind == "log":
                    self.log(payload)
                elif kind == "error":
                    self.status.configure(text="Failed")
                    messagebox.showerror("thOnk", payload)
                elif kind == "done":
                    self.finish(payload)
            if self.worker.running:
                self.root.after(self.POLL_MS, self.poll)
            else:
                self.thonk_btn.configure(text="Thonk", state="normal")

        def finish(self, info):
            self.last_output = info["output"]
            self.log("wrote %s of audio (%d grains) in %s, %.1fx realtime"
                     % (hms(info["seconds"]), info["grains"], hms(info["elapsed"]),
                        info["seconds"] / max(info["elapsed"], 1e-9)))
            if info["overflows"]:
                if info["overflow"] == "clip":
                    self.log("%d samples hit full scale and were clipped; "
                             "lower the level" % info["overflows"])
                else:
                    self.log("%d samples wrapped past full scale, as the original "
                             "did; lower the level or keep headroom automatically"
                             % info["overflows"])
            self.log("%s is complete and playable.%s"
                     % (os.path.basename(info["output"]),
                        " Stopped early." if info["stopped"] else ""))
            self.log("")
            self.status.configure(text="Done - %s of audio" % hms(info["seconds"]))
            self.progress.configure(value=1.0)
            for btn in (self.reveal_btn, self.play_btn):
                btn.configure(state="normal")

        def on_reveal(self):
            self._open(self.last_output, reveal=True)

        def on_play(self):
            self._open(self.last_output)

        def _open(self, path, reveal=False):
            if not path or not os.path.exists(path):
                return
            import subprocess
            try:
                if sys.platform == "darwin":
                    cmd = ["open", "-R", path] if reveal else ["open", path]
                elif os.name == "nt":  # pragma: no cover
                    cmd = ["explorer", "/select," + path] if reveal else ["start", path]
                else:  # pragma: no cover
                    cmd = ["xdg-open", os.path.dirname(path) if reveal else path]
                subprocess.Popen(cmd)
            except Exception as exc:
                messagebox.showerror("thOnk", str(exc))

        def on_fix(self):
            path = filedialog.askopenfilename(title="File to repair",
                                              filetypes=AUDIO_TYPES)
            if not path:
                return
            try:
                size, frames = fix_header(path)
            except Exception as exc:
                messagebox.showerror("thOnk", str(exc))
                return
            self.log("repaired %s: %d bytes%s"
                     % (os.path.basename(path), size,
                        "" if frames is None else ", %d frames" % frames))

        def on_about(self):
            messagebox.showinfo(
                "About thOnk",
                "A reimplementation of the granular engine from thOnk_0+2 "
                "(Arjen van der Schoot / Audio Ease, 1996-1999), whose interface "
                "was designed by =cw4t7abs.\n\n"
                "The original was a 68k/PowerPC application for classic Mac OS. "
                "This carries none of its code - the synthesis model is rebuilt "
                "from the description in its manual.")

        def on_quit(self):
            if self.worker and self.worker.running:
                if not messagebox.askokcancel(
                        "thOnk", "A render is running. Stop it and quit?\n\n"
                                 "The output file will be closed properly and "
                                 "stays playable."):
                    return
                self.worker.stop()
                self.worker.thread.join(timeout=10)
            self.root.destroy()

    root = tk.Tk()
    try:
        ttk.Style().theme_use("aqua" if sys.platform == "darwin" else "clam")
    except Exception:
        pass
    window = ThonkWindow(root)
    for arg in sys.argv[1:]:
        if not arg.startswith("-") and os.path.isfile(arg):
            window.in_var.set(arg)
            window.out_var.set(os.path.splitext(arg)[0] + "_thonk.aiff")
            break
    root.mainloop()
    return 0


# --------------------------------------------------------------------------
# headless check of the worker wiring
# --------------------------------------------------------------------------


def selftest(input_path, output_path, seconds=6.0):
    import time
    worker = Worker({
        "input": input_path, "output": output_path, "score": "hectic",
        "duration": seconds, "gain": 1.0, "overflow": "wrap",
        "autogain": True, "seed": 11, "block": 0.5,
    })
    worker.start()
    seen = {"progress": 0, "log": 0, "done": 0, "error": 0}
    done = None

    def take(events):
        nonlocal done
        for kind, payload in events:
            seen[kind] = seen.get(kind, 0) + 1
            if kind == "done":
                done = payload
            elif kind == "error":
                print("ERROR:", payload)

    while worker.running:
        take(worker.drain())
        time.sleep(0.05)
    worker.thread.join(timeout=30)
    take(worker.drain())  # events posted after the last poll
    print("events:", seen)
    assert seen["error"] == 0, "worker reported an error"
    assert seen["progress"] >= int(seconds / 0.5) - 1, "too few progress events"
    assert done and abs(done["seconds"] - seconds) < 0.1, "wrong output length"
    print("worker ok: %.1f s, %d grains, %.1fx realtime"
          % (done["seconds"], done["grains"],
             done["seconds"] / max(done["elapsed"], 1e-9)))

    # stopping mid-render must still close the file
    worker = Worker({
        "input": input_path, "output": output_path + ".stop.aiff",
        "score": "flowing", "duration": 600.0, "gain": 1.0, "overflow": "wrap",
        "autogain": True, "seed": 12, "block": 0.5,
    })
    worker.start()
    time.sleep(1.5)
    worker.stop()
    worker.thread.join(timeout=30)
    info = None
    for kind, payload in worker.drain():
        if kind == "done":
            info = payload
    assert info and info["stopped"] and info["seconds"] > 0, "stop did not finalize"
    print("stop ok: finalized %.1f s after an early stop" % info["seconds"])
    return 0


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        args = [a for a in sys.argv[1:] if not a.startswith("--")]
        sys.exit(selftest(args[0], args[1]))
    sys.exit(run_gui())
