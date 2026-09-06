"""Export a Loudstone cuboid-model JSON file as a self-contained .bbmodel."""

from __future__ import annotations

import argparse
import base64
import json
import math
import struct
import uuid
import zlib
from pathlib import Path

TILE = 16
FACE_NAMES = ("north", "east", "south", "west", "up", "down")


def png(width: int, height: int, pixels: bytes) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    rows = b"".join(b"\0" + pixels[y * width * 4 : (y + 1) * width * 4] for y in range(height))
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows, 9))
        + chunk(b"IEND", b"")
    )


def tile_pixels(material: str, tint: tuple[float, float, float]) -> bytes:
    bases = {
        "skin": (76, 140, 82),
        "face": (76, 140, 82),
        "shirt": (64, 96, 132),
    }
    base = bases[material]
    out = bytearray()
    for y in range(TILE):
        for x in range(TILE):
            noise = 0.82 + ((x * 17 + y * 31 + x * y * 3) % 29) / 145.0
            color = [min(255, int(base[i] * tint[i] * noise)) for i in range(3)]
            if material == "face":
                if (x in (4, 5) and y in (5, 6)) or (x in (11, 12) and y in (5, 6)):
                    color = [20, 24, 20]
                if x == 4 and y == 5:
                    color = [150, 18, 18]
                if 5 <= x <= 12 and y in (11, 12):
                    color = [30, 38, 30]
            out.extend((*color, 255))
    return bytes(out)


def write_runtime_ron(model: dict, path: Path) -> None:
    material_names = {"skin": "skin", "face": "face", "shirt": "shirt"}
    def ron_value(value: object) -> str:
        if isinstance(value, list):
            return "(" + ", ".join(str(item) for item in value) + ")"
        return json.dumps(value)

    lines = ["(", f"    name: {json.dumps(model['name'])},", "    parts: ["]
    for part in model["parts"]:
        lines.append("        Part(")
        for key in ("name", "pivot", "from", "to", "bend", "tint"):
            if key in part:
                lines.append(f"            {key}: {ron_value(part[key])},")
        lines.append(f"            material: {material_names[part['material']]},")
        if "front" in part:
            lines.append(f"            front: Some({material_names[part['front']]}),")
        if "channel" in part:
            lines.append(f"            channel: Some({json.dumps(part['channel'])}),")
        lines.append("        ),")
    lines.extend(["    ],", ")", ""])
    path.write_text("\n".join(lines), encoding="utf-8")


def export(source: Path, output: Path) -> None:
    model = json.loads(source.read_text(encoding="utf-8"))
    write_runtime_ron(model, source.with_suffix(".ron"))
    keys: list[tuple[str, tuple[float, float, float]]] = []
    for part in model["parts"]:
        tint = tuple(part["tint"])
        for material in (part["material"], part.get("front")):
            if material is not None and (material, tint) not in keys:
                keys.append((material, tint))

    width = TILE * len(keys)
    pixels = bytearray(width * TILE * 4)
    for tile_index, key in enumerate(keys):
        tile = tile_pixels(*key)
        for y in range(TILE):
            target = (y * width + tile_index * TILE) * 4
            pixels[target : target + TILE * 4] = tile[y * TILE * 4 : (y + 1) * TILE * 4]
    texture_png = png(width, TILE, bytes(pixels))
    texture_path = output.with_name("zombie_texture.png")
    texture_path.write_bytes(texture_png)

    elements = []
    outliner = []
    namespace = uuid.UUID("34db5426-4850-4b54-89bd-131cc7b8aa70")
    for index, part in enumerate(model["parts"]):
        pivot = [v * 16.0 for v in part["pivot"]]
        start = [(part["pivot"][i] + part["from"][i]) * 16.0 for i in range(3)]
        end = [(part["pivot"][i] + part["to"][i]) * 16.0 for i in range(3)]
        element_id = str(uuid.uuid5(namespace, part["name"]))
        tint = tuple(part["tint"])
        normal_tile = keys.index((part["material"], tint))
        front_tile = keys.index((part.get("front", part["material"]), tint))
        faces = {}
        for face in FACE_NAMES:
            tile_index = front_tile if face == "east" else normal_tile
            u = tile_index * TILE
            faces[face] = {"uv": [u, 0, u + TILE, TILE], "texture": 0}
        element = {
            "name": part["name"],
            "box_uv": False,
            "render_order": "default",
            "locked": False,
            "from": start,
            "to": end,
            "autouv": 0,
            "color": index % 8,
            "origin": pivot,
            "faces": faces,
            "type": "cube",
            "uuid": element_id,
        }
        bend = part.get("bend", 0.0)
        if bend:
            element["rotation"] = [0, 0, math.degrees(bend)]
        elements.append(element)
        outliner.append(element_id)

    texture_id = str(uuid.uuid5(namespace, "zombie_texture"))
    project = {
        "meta": {"format_version": "5.0", "model_format": "free", "box_uv": False},
        "name": model["name"],
        "model_identifier": f"loudstone:{model['name']}",
        "visible_box": [2, 2, 0],
        "resolution": {"width": width, "height": TILE},
        "elements": elements,
        "outliner": outliner,
        "textures": [
            {
                "path": texture_path.name,
                "name": texture_path.name,
                "folder": "",
                "namespace": "",
                "id": "0",
                "particle": False,
                "render_mode": "default",
                "visible": True,
                "mode": "bitmap",
                "saved": True,
                "uuid": texture_id,
                "uv_width": width,
                "uv_height": TILE,
                "source": "data:image/png;base64," + base64.b64encode(texture_png).decode("ascii"),
            }
        ],
    }
    output.write_text(json.dumps(project, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    export(args.source, args.output)
