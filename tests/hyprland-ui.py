#!/usr/bin/env python3
"""Opt-in native Wayland responsiveness check; requires Hyprland 0.57+ and hyprctl.

Usage: python3 tests/hyprland-ui.py /absolute/path/to/ravel
Only the isolated test window is moved. Pairing state is never read.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import threading
import time


with tempfile.TemporaryDirectory(prefix="ravel-ui-check-") as directory:
    state = Path(directory) / "peers.json"
    state.write_text('{"peers":{},"autostart_prompted":true}')
    state.chmod(0o600)
    environment = dict(os.environ, WAYLAND_DEBUG="1")
    environment.pop("DISPLAY", None)
    child = subprocess.Popen(
        [sys.argv[1], "--foreground", "--port", "0", "--state-dir", directory],
        env=environment,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    events = []

    def read_events():
        for line in child.stderr:
            # Keep only compositor liveness messages; never keyboard events.
            match = re.search(r"xdg_wm_base#\d+\.(ping|pong)\((\d+)\)", line)
            if match:
                events.append((time.monotonic(), *match.groups()))

    reader = threading.Thread(target=read_events, daemon=True)
    reader.start()
    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            clients = json.loads(subprocess.check_output(["hyprctl", "clients", "-j"]))
            if any(client.get("pid") == child.pid for client in clients):
                break
            if child.poll() is not None:
                raise RuntimeError("Isolated Ravel exited before mapping its window")
            time.sleep(0.1)
        else:
            raise RuntimeError("Isolated window did not map")
        subprocess.run(
            ["hyprctl", "eval", 'hl.dispatch(hl.dsp.window.move({workspace = '
             f'"special:ravel-qa", window = "pid:{child.pid}", follow = false}}))'],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        started = time.monotonic()
        time.sleep(30)
        observed = [event for event in events if event[0] >= started]
        pings = [event for event in observed if event[1] == "ping"]
        pongs = [event for event in observed if event[1] == "pong"]
        print(f"Offscreen for 30s: {len(pings)} compositor pings, {len(pongs)} replies")
        assert child.poll() is None, "Ravel exited during the check"
        assert len(pings) >= 2, "No continuing ping dispatch: blocked or inconclusive"
        assert len(pongs) == len(pings), "Compositor pings went unanswered"
        assert pongs[-1][0] >= started + 25, "GUI stopped responding while offscreen"
    finally:
        child.terminate()
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
        reader.join(timeout=1)
