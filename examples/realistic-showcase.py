"""Authoring tool for the Realistic showcase (material graph note, section 5.9).

Python is not an application dependency. Run it from the repository root:

    python3 examples/realistic-showcase.py

It draws the original label image analytically (no fonts, no external artwork) into
assets/examples/realistic-showcase-label.png and writes examples/realistic-showcase.json, a
command script. The script is the editable authority; it builds the design through the same
commands as the UI, the CLI and MCP:

    solidcraft-cli run examples/realistic-showcase.json --save target/realistic-showcase.sc

Every value is an illustrative visual estimate, not measured optical data. All dimensions
are millimetres; every part rests on the studio floor (z = 0).

Copyright 2026 SolidCraft contributors. SPDX-License-Identifier: MIT OR Apache-2.0
"""
import hashlib
import json
import math
import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LABEL = "assets/examples/realistic-showcase-label.png"
SCRIPT = "examples/realistic-showcase.json"

# ---------------------------------------------------------------------------------------------
# The label: an original 1024 x 512 RGBA image drawn with signed distances (one sample per
# pixel, coverage from the distance), so the bytes are deterministic.

W, H = 1024, 512
PAPER = (242, 238, 228)
INK = (30, 34, 38)
ACCENT = (224, 102, 42)
GREY = (150, 152, 150)

# Stroke glyphs on a 4 x 6 grid, y down.
GLYPHS = {
    "R": [[(0, 6), (0, 0), (3, 0), (4, 1), (4, 2), (3, 3), (0, 3)], [(2, 3), (4, 6)]],
    "E": [[(4, 0), (0, 0), (0, 6), (4, 6)], [(0, 3), (3, 3)]],
    "A": [[(0, 6), (2, 0), (4, 6)], [(0.75, 3.8), (3.25, 3.8)]],
    "L": [[(0, 0), (0, 6), (4, 6)]],
    "I": [[(2, 0), (2, 6)], [(1, 0), (3, 0)], [(1, 6), (3, 6)]],
    "S": [[(4, 1), (3, 0), (1, 0), (0, 1), (0, 2), (1, 3), (3, 3), (4, 4), (4, 5), (3, 6), (1, 6), (0, 5)]],
    "T": [[(0, 0), (4, 0)], [(2, 0), (2, 6)]],
    "C": [[(4, 1), (3, 0), (1, 0), (0, 1), (0, 5), (1, 6), (3, 6), (4, 5)]],
}


def seg_dist(px, py, ax, ay, bx, by):
    dx, dy = bx - ax, by - ay
    t = ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)
    t = min(1.0, max(0.0, t))
    return math.hypot(px - ax - t * dx, py - ay - t * dy)


def rounded_rect(px, py, x0, y0, x1, y1, r):
    """Signed distance to a rounded rectangle (negative inside)."""
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    hx, hy = (x1 - x0) / 2 - r, (y1 - y0) / 2 - r
    qx, qy = abs(px - cx) - hx, abs(py - cy) - hy
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - r


def text_segments(text, x, y, cap, gap):
    s = cap / 6
    out = []
    for ch in text:
        for stroke in GLYPHS[ch]:
            for (ax, ay), (bx, by) in zip(stroke, stroke[1:]):
                out.append((x + ax * s, y + ay * s, x + bx * s, y + by * s))
        x += 4 * s + gap
    return out


TITLE = text_segments("REALISTIC", 380, 160, 64, 20)
TITLE_BOX = (360, 140, 1000, 244)


def cover(d):
    return min(1.0, max(0.0, 0.5 - d))


def pixel(px, py):
    """Straight RGBA of one pixel centre, painted back to front."""
    plate = cover(rounded_rect(px, py, 16, 16, W - 16, H - 16, 56))
    if plate <= 0:
        return (0, 0, 0, 0)
    rgb = list(PAPER)

    def paint(colour, a):
        for k in range(3):
            rgb[k] += (colour[k] - rgb[k]) * a

    # Inner border.
    paint(INK, cover(abs(rounded_rect(px, py, 40, 40, W - 40, H - 40, 34)) - 4))
    # The mark: a ring around an accent hexagon.
    cx, cy = 196, 256
    ring = abs(math.hypot(px - cx, py - cy) - 104) - 14
    paint(INK, cover(ring))
    paint(ACCENT, cover(polygon(px, py, HEXAGON)))
    # The title.
    x0, y0, x1, y1 = TITLE_BOX
    if x0 <= px <= x1 and y0 <= py <= y1:
        d = min(seg_dist(px, py, *s) for s in TITLE)
        paint(INK, cover(d - 7))
    # Two grey rules and an accent bar.
    paint(GREY, cover(rounded_rect(px, py, 380, 276, 924, 288, 6)))
    paint(GREY, cover(rounded_rect(px, py, 380, 310, 760, 322, 6)))
    paint(ACCENT, cover(rounded_rect(px, py, 380, 350, 580, 376, 8)))
    return (round(rgb[0]), round(rgb[1]), round(rgb[2]), round(255 * plate))


HEXAGON = [(196 + 58 * math.cos(math.pi / 3 * k), 256 + 58 * math.sin(math.pi / 3 * k)) for k in range(6)]


def polygon(px, py, corners):
    """Signed distance to a convex polygon's edge lines (negative inside)."""
    gx = sum(c[0] for c in corners) / len(corners)
    gy = sum(c[1] for c in corners) / len(corners)
    d = -math.inf
    for (ax, ay), (bx, by) in zip(corners, corners[1:] + corners[:1]):
        nx, ny = by - ay, ax - bx
        n = math.hypot(nx, ny)
        if (gx - ax) * nx + (gy - ay) * ny > 0:
            nx, ny = -nx, -ny
        d = max(d, ((px - ax) * nx + (py - ay) * ny) / n)
    return d


def png(width, height, rows):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    raw = b"".join(b"\0" + bytes(row) for row in rows)
    head = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", head) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def label_png():
    rows = []
    for y in range(H):
        row = []
        for x in range(W):
            row.extend(pixel(x + 0.5, y + 0.5))
        rows.append(row)
    return png(W, H, rows)


# ---------------------------------------------------------------------------------------------
# The scene.


class Script:
    def __init__(self):
        self.commands = []

    def cmd(self, command, **params):
        self.commands.append({"command": command, "params": params})

    def rounded_block(self, name, centre, size, radius, top_fillet):
        """A rounded-rectangle block on the floor with its top edges filleted."""
        (cx, cy), (w, d, h) = centre, size
        self.cmd("sketch.create", name=f"{name} profile", plane={"origin": [cx, cy, 0], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]})
        self.cmd("sketch.rectangle.rounded", p0=[-w / 2, -d / 2], p1=[w / 2, d / 2], radius=radius)
        self.cmd("sketch.finish")
        self.cmd("solid.extrude", distance=h, operation="new", body_name=name, name=name)
        if top_fillet:
            k = radius * (1 - math.cos(math.pi / 4))
            edges = [[cx, cy - d / 2, h], [cx, cy + d / 2, h], [cx - w / 2, cy, h], [cx + w / 2, cy, h]]
            edges += [[cx + sx * (w / 2 - k), cy + sy * (d / 2 - k), h] for sx in (-1, 1) for sy in (-1, 1)]
            self.cmd("solid.fillet", edges=edges, radius=top_fillet)


def graphs(label_sha):
    brushed = {
        "name": "Brushed aluminium",
        "nodes": [
            {"id": "uv", "kind": "texcoord.planar", "params": {"space": "object"}, "inputs": {"origin": [0, 0, 0], "u": [1, 0, 0], "v": [0, 1, 0]}},
            {"id": "lines", "kind": "pattern.brushed",
             "inputs": {"texcoord": {"from": "uv"}, "density_per_mm": 40, "streak_mm": 6, "contrast": 0.55, "jitter": 0.3, "seed": 5}},
            {"id": "rough", "kind": "remap", "inputs": {"in": {"from": "lines"}, "out_min": 0.22, "out_max": 0.30}},
            {"id": "tint", "kind": "remap", "inputs": {"in": {"from": "lines"}, "out_min": 0.96, "out_max": 1.03}},
            {"id": "base", "kind": "multiply", "inputs": {"a": [0.91, 0.92, 0.92], "b": {"from": "tint"}}},
            {"id": "surface", "kind": "standard_surface", "inputs": {
                "base_color": {"from": "base", "fallback": [0.91, 0.92, 0.92]}, "base_metalness": 1,
                "specular_roughness": {"from": "rough", "fallback": 0.26}, "specular_roughness_anisotropy": 0.7,
                "geometry_tangent": {"from": "lines.tangent"}}},
        ],
        "output": "surface",
    }
    epoxy = {
        "name": "Black epoxy, clear coated",
        "nodes": [
            {"id": "p", "kind": "geometry.position", "params": {"space": "object"}},
            {"id": "edm", "kind": "pattern.noise", "params": {"kind": "cellular", "dimensions": 3, "octaves": 2},
             "inputs": {"position": {"from": "p"}, "scale_mm": 0.04, "seed": 11}},
            {"id": "edm_r", "kind": "remap", "inputs": {"in": {"from": "edm"}, "out_min": 0.40, "out_max": 0.52}},
            {"id": "surface", "kind": "standard_surface", "inputs": {
                "base_color": [0.022, 0.022, 0.024], "specular_roughness": {"from": "edm_r", "fallback": 0.46}, "specular_ior": 1,
                "coat_weight": 1, "coat_roughness": 0.08, "coat_ior": 1.5}},
        ],
        "output": "surface",
    }
    # The label's planar frame: the image's top row lies along the plate's far edge, so the print
    # reads upright from the front. One repeat spans the label (60 x 30 mm, clamped outside).
    lx, ly = PLATE[0] - 20, PLATE[1] + 15
    carbon = {
        "name": "Carbon-fibre twill with label",
        "interface": [{"name": "label", "type": "texture"}],
        "nodes": [
            {"id": "uv", "kind": "texcoord.planar", "params": {"space": "object"}, "inputs": {"origin": [0, 0, 0], "u": [0.7071, 0.7071, 0], "v": [-0.7071, 0.7071, 0]}},
            {"id": "weave", "kind": "pattern.weave", "params": {"kind": "twill_2x2"},
             "inputs": {"texcoord": {"from": "uv"}, "pitch_mm": [2.0, 2.0], "crown": 1, "fibres": 9, "fibre_contrast": 0.35}},
            {"id": "albedo", "kind": "remap", "inputs": {"in": {"from": "weave"}, "out_min": 0.018, "out_max": 0.075}},
            {"id": "rough", "kind": "remap", "inputs": {"in": {"from": "weave"}, "out_min": 0.42, "out_max": 0.28}},
            {"id": "bump", "kind": "normal.from_height", "inputs": {"height": {"from": "weave"}, "relief_mm": 0.02}},
            {"id": "carbon", "kind": "standard_surface", "inputs": {
                "base_color": {"from": "albedo", "fallback": [0.035, 0.035, 0.038]}, "specular_roughness": {"from": "rough", "fallback": 0.35}, "specular_ior": 1,
                "geometry_normal": {"from": "bump"},
                "coat_weight": 1, "coat_roughness": 0.05, "coat_ior": 1.5}},
            {"id": "frame", "kind": "texcoord.planar", "params": {"space": "object"},
             "inputs": {"origin": [lx, ly, 0], "u": [60, 0, 0], "v": [0, -30, 0]}},
            {"id": "print", "kind": "image", "params": {"space": "srgb", "wrap": "clamp", "channel": "a"},
             "inputs": {"texcoord": {"from": "frame"}, "file": {"interface": "label"}}},
            {"id": "paper", "kind": "standard_surface", "inputs": {
                "base_color": {"from": "print.color", "fallback": [0.95, 0.93, 0.89]}, "specular_roughness": 0.5, "specular_ior": 1.5}},
            {"id": "surface", "kind": "surface.mix", "inputs": {"fg": {"from": "paper"}, "bg": {"from": "carbon"}, "mix": {"from": "print.alpha", "fallback": 0}}},
        ],
        "output": "surface",
    }
    silicone = {
        "name": "Clear silicone",
        "nodes": [
            {"id": "surface", "kind": "standard_surface", "inputs": {
                "base_color": [0.96, 0.98, 0.99], "specular_roughness": 0.12, "specular_ior": 1.41,
                "transmission_weight": 1, "transmission_color": [0.80, 0.92, 0.95]}},
        ],
        "output": "surface",
    }
    return {"brushed-aluminium": brushed, "black-epoxy": epoxy, "carbon-twill": carbon, "clear-silicone": silicone}


# Placement (centres on the floor, mm). Back row: Sample Plate, brushed lid, carbon plate.
# Front row: polished steel cylinder and sphere, coated epoxy box, silicone dome.
SAMPLE = (-150, 30)  # the Sample Plate's corner; it is 80 x 50 mm
LID = (-10, 55)
PLATE = (115, 50)
CYLINDER = (-125, -45)
SPHERE = (-75, -45)
EPOXY = (-5, -45)
DOME = (85, -45)


def sample_plate(s):
    """The built-in Sample Plate (crates/engine/src/sample.rs), built at the origin, then moved."""
    s.cmd("parameters.change", name="width", expression="80 mm", comment="plate width")
    s.cmd("parameters.change", name="depth", expression="50 mm")
    s.cmd("parameters.change", name="thickness", expression="8 mm")
    s.cmd("sketch.create", plane="XY", name="Base")
    s.cmd("sketch.rectangle.two_point", p0=[0, 0], p1=[80, 50])
    s.cmd("sketch.constraint.coincident", a="p1", b="origin")
    s.cmd("sketch.dimension", entities=["l1"], value="width")
    s.cmd("sketch.dimension", entities=["l2"], value="depth")
    s.cmd("sketch.finish")
    s.cmd("solid.extrude", distance="thickness", body_name="Sample Plate")
    s.cmd("solid.fillet", edges=[[0, 0, 4], [80, 0, 4], [80, 50, 4], [0, 50, 4]], radius="6 mm")
    s.cmd("sketch.create", plane="XY", name="Holes")
    s.cmd("sketch.circle.center", center=[12, 25], diameter=8)
    s.cmd("sketch.circle.center", center=[68, 25], diameter=8)
    s.cmd("sketch.finish")
    s.cmd("solid.extrude", distance="thickness", operation="cut", name="Holes")
    s.cmd("sketch.create", plane="XY", offset="thickness", name="Boss")
    s.cmd("sketch.circle.center", center=[40, 25], diameter=24)
    s.cmd("sketch.finish")
    s.cmd("solid.extrude", distance=14, operation="join", name="Boss")
    s.cmd("sketch.create", plane="XY", offset="thickness + 14 mm", name="Bore")
    s.cmd("sketch.circle.center", center=[40, 25], diameter=12)
    s.cmd("sketch.finish")
    s.cmd("solid.extrude", distance="thickness + 14 mm", direction="negative", operation="cut", name="Bore")
    s.cmd("solid.move", bodies=["Sample Plate"], translate=[SAMPLE[0], SAMPLE[1], 0])


def scene(label_sha):
    s = Script()
    s.cmd("file.new", name="Realistic showcase")
    s.cmd("document.metadata.set", values={
        "license": "MIT OR Apache-2.0",
        "author": "SolidCraft contributors",
        "project": "Realistic view showcase",
        "description": "Original example design for the Realistic view: brushed, polished and coated "
                       "finishes, a carbon-fibre twill with a printed label, a clear silicone dome and the "
                       "Sample Plate. Illustrative visual estimates, not measured optical data.",
    })

    sample_plate(s)
    s.rounded_block("Brushed lid", LID, (90, 60, 10), 8, 3)
    s.rounded_block("Carbon plate", PLATE, (110, 70, 4), 6, 0)
    s.cmd("solid.cylinder", body_name="Steel cylinder", base=[CYLINDER[0], CYLINDER[1], 0], radius=15, height=40, axis="Z", operation="new")
    s.cmd("solid.sphere", body_name="Steel sphere", center=[SPHERE[0], SPHERE[1], 18], radius=18, operation="new")
    s.rounded_block("Epoxy box", EPOXY, (54, 40, 26), 7, 4)
    s.cmd("solid.sphere", body_name="Silicone dome", center=[DOME[0], DOME[1], 0], radius=24, operation="new")
    s.cmd("solid.box", corner=[DOME[0] - 26, DOME[1] - 26, -26], length=52, width=52, height=26,
          operation="cut", targets=["Silicone dome"], name="Dome base")

    s.cmd("appearance.texture.add", path=LABEL, name="Showcase label")
    for gid, graph in graphs(label_sha).items():
        s.cmd("appearance.graph.create", id=gid, name=graph["name"], graph=graph)

    s.cmd("appearance.assign", bodies=["Brushed lid"], graph="brushed-aluminium", name="Brushed aluminium")
    s.cmd("appearance.assign", bodies=["Steel cylinder", "Steel sphere"], color="#c4c7cb",
          pbr={"metallic": 1, "roughness": 0.08, "dielectric_ior": 1.5}, name="Polished steel")
    s.cmd("appearance.assign", bodies=["Epoxy box"], graph="black-epoxy", name="Black epoxy, clear coated")
    s.cmd("appearance.assign", bodies=["Carbon plate"], graph="carbon-twill", values={"label": {"texture": label_sha}},
          name="Carbon twill, labelled")
    s.cmd("appearance.assign", bodies=["Silicone dome"], graph="clear-silicone", name="Clear silicone")
    # The Sample Plate: a metal PBR look on the body and a plain colour on the boss's top face.
    sx, sy = SAMPLE
    s.cmd("appearance.assign", bodies=["Sample Plate"], color="#c9ccd1", pbr={"metallic": 1, "roughness": 0.25, "dielectric_ior": 1.5}, name="Machined steel")
    s.cmd("appearance.assign", body="Sample Plate", faces=[[sx + 40, sy + 25 + 9, 22]], color="#2f6db5")

    # Named views: isometric front-left, a low three-quarter view from the back, and a close-up
    # of the label and the dome.
    for name, target, yaw, pitch, distance in [
        ("Showcase: isometric front-left", [-10, 5, 12], math.pi / 4, 0.6154797086703874, 520),
        ("Showcase: low back three-quarter", [-10, 5, 10], -2.35, 0.26, 500),
        ("Showcase: label and dome", [100, 0, 6], 0.55, 0.62, 260),
    ]:
        s.cmd("view.save", name=name, camera={"target": target, "yaw": yaw, "pitch": pitch, "distance": distance, "fov": 0.6})
    s.cmd("view.apply", view="Showcase: isometric front-left")
    return {"commands": s.commands}


def main():
    data = label_png()
    path = ROOT / LABEL
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    sha = hashlib.sha256(data).hexdigest()
    text = json.dumps(scene(sha), indent=1) + "\n"
    (ROOT / SCRIPT).write_text(text, encoding="utf-8", newline="\n")
    print(f"{LABEL}: {len(data)} bytes, sha256 {sha}")


if __name__ == "__main__":
    main()
