#!/bin/bash
# Build a fully self-contained thOnk.app on macOS - Python, numpy and Tk all
# inside the bundle, so it runs on a Mac with no Python installed at all.
#
# The thOnk.app shipped alongside this script does not need any of it: that one
# is a launcher that borrows a Python already on the machine. Use this when you
# want something you can hand to someone else.
#
#   ./build_app.sh
#
# Result: dist/thOnk.app
#
# Needs a Python with tkinter (python.org installer or `brew install python
# python-tk`). Apple's /usr/bin/python3 works for running the app but is a poor
# base to build from, since PyInstaller cannot always collect its Tk.

set -euo pipefail
cd "$(dirname "$0")"

PY="${PYTHON:-python3}"
echo "==> using $("$PY" -c 'import sys; print(sys.version.split()[0], sys.executable)')"

"$PY" -c 'import tkinter' 2>/dev/null || {
    echo "error: this Python has no tkinter. Try: brew install python python-tk" >&2
    echo "       then re-run as: PYTHON=/opt/homebrew/bin/python3 ./build_app.sh" >&2
    exit 1
}

echo "==> installing build dependencies into a local venv"
rm -rf .buildvenv
"$PY" -m venv --system-site-packages .buildvenv
VPY=".buildvenv/bin/python"
"$VPY" -m pip install --quiet --upgrade pip
"$VPY" -m pip install --quiet numpy pyinstaller

echo "==> checking the engine runs before bundling it"
"$VPY" - <<'PY'
import numpy as np, os, struct, tempfile, sys
sys.path.insert(0, os.getcwd())
import thonk
sr = 22050
x = (np.random.default_rng(0).normal(0, 0.2, sr) * np.linspace(1, 0, sr)).astype(np.float32)
tmp = tempfile.mkdtemp()
out = os.path.join(tmp, "smoke.aiff")
s = thonk.Session(x, sr, out, score="flowing", duration=2.0, autogain=True, seed=1)
for _ in s.run():
    pass
assert s.written_seconds > 1.9, "engine smoke test failed"
print("    engine ok: %.1f s rendered" % s.written_seconds)
PY

echo "==> building the bundle"
rm -rf build dist
"$VPY" -m PyInstaller \
    --noconfirm --clean --windowed \
    --name thOnk \
    --icon thOnk.app/Contents/Resources/thOnk.icns \
    --osx-bundle-identifier net.granular.thonk \
    --add-data "thOnk_icon.png:." \
    --add-data "README.md:." \
    --hidden-import tkinter \
    --collect-submodules numpy \
    thonk_gui.py

test -d dist/thOnk.app || { echo "error: no bundle produced" >&2; exit 1; }

echo "==> signing locally so Gatekeeper lets it run on this machine"
codesign --force --deep --sign - dist/thOnk.app || \
    echo "    (ad-hoc signing failed; right-click the app and choose Open the first time)"

echo
echo "done: dist/thOnk.app"
echo "If macOS refuses to open it after copying to another Mac, run:"
echo "    xattr -dr com.apple.quarantine /Applications/thOnk.app"
