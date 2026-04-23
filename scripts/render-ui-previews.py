from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


PRESETS = [
    ("dashboard-narrow", "dashboard", 90, 28),
    ("dashboard-regular", "dashboard", 120, 34),
    ("dashboard-wide", "dashboard", 160, 40),
    ("actions-regular", "actions", 120, 34),
    ("providers-regular", "providers", 120, 34),
]


def pick_font(size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    candidates = [
        Path("C:/Windows/Fonts/consola.ttf"),
        Path("C:/Windows/Fonts/consolab.ttf"),
        Path("C:/Windows/Fonts/CascadiaMono.ttf"),
        Path("C:/Windows/Fonts/lucon.ttf"),
    ]
    for candidate in candidates:
        if candidate.exists():
            return ImageFont.truetype(str(candidate), size=size)
    return ImageFont.load_default()


def render_text_to_png(text: str, output: Path) -> None:
    lines = text.splitlines() or [""]
    font = pick_font(18)
    probe = Image.new("RGB", (10, 10))
    draw = ImageDraw.Draw(probe)
    bbox = draw.textbbox((0, 0), "M", font=font)
    cell_w = max(10, bbox[2] - bbox[0] + 1)
    cell_h = max(18, bbox[3] - bbox[1] + 6)
    width = max(len(line) for line in lines)
    image = Image.new(
        "RGB",
        (width * cell_w + 32, len(lines) * cell_h + 32),
        color=(11, 16, 22),
    )
    draw = ImageDraw.Draw(image)
    for index, line in enumerate(lines):
        draw.text((16, 16 + index * cell_h), line, fill=(228, 236, 245), font=font)
    image.save(output)


def render_grid_to_png(payload: dict, output: Path) -> None:
    width = payload["width"]
    height = payload["height"]
    cells = payload["cells"]
    regular_font = pick_font(18)
    bold_font = pick_font(18)
    probe = Image.new("RGB", (10, 10))
    probe_draw = ImageDraw.Draw(probe)
    bbox = probe_draw.textbbox((0, 0), "M", font=regular_font)
    cell_w = max(10, bbox[2] - bbox[0] + 1)
    cell_h = max(18, bbox[3] - bbox[1] + 6)
    image = Image.new("RGB", (width * cell_w + 32, height * cell_h + 32), color=(11, 16, 22))
    draw = ImageDraw.Draw(image)

    for index, cell in enumerate(cells):
        x = index % width
        y = index // width
        left = 16 + x * cell_w
        top = 16 + y * cell_h
        bg = tuple(cell["bg"])
        fg = tuple(cell["fg"])
        symbol = cell["symbol"]
        draw.rectangle((left, top, left + cell_w, top + cell_h), fill=bg)
        if symbol and symbol != " ":
            draw.text(
                (left, top),
                symbol,
                fill=fg,
                font=bold_font if cell["bold"] else regular_font,
            )

    image.save(output)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output-dir", default="artifacts/ui-previews")
    parser.add_argument("--live", action="store_true")
    args = parser.parse_args()

    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    for name, section, width, height in PRESETS:
        command = [
            "cargo",
            "run",
            "--quiet",
            "--bin",
            "ui_preview",
            "--",
            "--section",
            section,
            "--width",
            str(width),
            "--height",
            str(height),
        ]
        if not args.live:
            command.append("--sample")
        rendered = subprocess.run(
            command,
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
        ).stdout
        (output_dir / f"{name}.txt").write_text(rendered, encoding="utf-8")
        command.append("--format")
        command.append("json")
        payload = subprocess.run(
            command,
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
        ).stdout
        render_grid_to_png(json.loads(payload), output_dir / f"{name}.png")


if __name__ == "__main__":
    main()
