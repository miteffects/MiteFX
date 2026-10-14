#!/usr/bin/env python3
"""Measures what the compositor actually puts on screen for a window region (GNOME/Mutter).

Plugin-side frame counts can look perfect while the user sees lag; this counts distinct frames
delivered by Mutter's screen cast of the area, which is what reaches the display.

    python3 tools/onscreen_fps.py X Y W H [seconds]     (X11/XWayland root coordinates)
"""
import sys, time
import gi
gi.require_version("Gio", "2.0"); gi.require_version("Gst", "1.0")
from gi.repository import Gio, GLib, Gst
import numpy as np

x, y, w, h = (int(v) for v in sys.argv[1:5])
seconds = float(sys.argv[5]) if len(sys.argv) > 5 else 4.0
scale = float(sys.argv[6]) if len(sys.argv) > 6 else 1.0  # X coords → Mutter logical coords

bus = Gio.bus_get_sync(Gio.BusType.SESSION)
def call(path, iface, method, args, rtype):
    return bus.call_sync("org.gnome.Mutter.ScreenCast", path, iface, method, args,
                         GLib.VariantType(rtype) if rtype else None, Gio.DBusCallFlags.NONE, -1, None)

session = call("/org/gnome/Mutter/ScreenCast", "org.gnome.Mutter.ScreenCast", "CreateSession",
               GLib.Variant("(a{sv})", ({},)), "(o)").unpack()[0]
props = {"cursor-mode": GLib.Variant("u", 0)}
stream = call(session, "org.gnome.Mutter.ScreenCast.Session", "RecordArea",
              GLib.Variant("(iiiia{sv})", (int(x / scale), int(y / scale), int(w / scale), int(h / scale), props)), "(o)").unpack()[0]

node = {}
def on_signal(conn, sender, path, iface, signal, params, _):
    node["id"] = params.unpack()[0]
bus.signal_subscribe(None, "org.gnome.Mutter.ScreenCast.Stream", "PipeWireStreamAdded", stream, None,
                     Gio.DBusSignalFlags.NONE, on_signal, None)
call(session, "org.gnome.Mutter.ScreenCast.Session", "Start", None, None)
loop = GLib.MainLoop()
ctx = loop.get_context()
t_end = time.time() + 5
while "id" not in node and time.time() < t_end:
    ctx.iteration(True)
if "id" not in node:
    sys.exit("no PipeWire stream")

Gst.init(None)
pipe = Gst.parse_launch(f"pipewiresrc path={node['id']} always-copy=true ! videoscale method=nearest-neighbour ! video/x-raw,width=320 ! videoconvert ! video/x-raw,format=GRAY8 ! appsink name=sink emit-signals=false sync=false max-buffers=1000")
sink = pipe.get_by_name("sink")
pipe.set_state(Gst.State.PLAYING)
frames, start = [], time.time()
while time.time() - start < seconds:
    sample = sink.emit("try-pull-sample", int(0.05 * Gst.SECOND))
    if sample is None:
        continue
    buf = sample.get_buffer()
    caps = sample.get_caps().get_structure(0)
    ok, info = buf.map(Gst.MapFlags.READ)
    data = np.frombuffer(info.data, dtype=np.uint8).copy()
    buf.unmap(info)
    frames.append((time.time(), data))
pipe.set_state(Gst.State.NULL)
call(session, "org.gnome.Mutter.ScreenCast.Session", "Stop", None, None)

changed = [frames[0][0]] if frames else []
for (t0, a), (t1, b) in zip(frames, frames[1:]):
    if a.shape != b.shape or np.any(a != b):
        changed.append(t1)
gaps = np.diff(changed) * 1000 if len(changed) > 2 else np.array([0.0])
import os
if os.environ.get("CHANGES_OUT"):
    with open(os.environ["CHANGES_OUT"], "w") as f:
        f.write("\n".join(f"{t:.6f}" for t in changed))
print(f"delivered {len(frames)} frames, {len(changed)} distinct in {seconds:.1f} s "
      f"-> {len(changed) / seconds:.1f} visible updates/s; gap median {np.median(gaps):.1f} ms, "
      f"p95 {np.percentile(gaps, 95):.1f} ms, max {gaps.max():.1f} ms")
