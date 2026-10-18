#!/usr/bin/env python3
"""Turn the TUI's demo snapshots into docs/images/demo.gif.

The frames come from the `demo_walks_a_prompt_through_the_queue` test, so the
GIF shows the real home screen with made-up sessions and needs no live agents:

    VALK_SNAPSHOT_DIR=/tmp/valk-frames cargo test -p valkyrie-tui --lib demo_walks
    python3 docs/demo/frames.py /tmp/valk-frames docs/images/demo.gif

Needs rsvg-convert (librsvg), ImageMagick's `magick`, and ffmpeg.
"""

import json
import subprocess
import sys
import tempfile
from html import escape
from pathlib import Path

CELL_W, CELL_H = 8.4, 18.0
PAD, TOP = 14.0, 44.0
BG, FG = "#21222c", "#f8f8f2"
FONT = ("'JetBrainsMono Nerd Font Mono','JetBrains Mono','DejaVu Sans Mono',"
        "monospace")

# Each frame: (snapshot, seconds on screen, caption). There's no sound in a GIF,
# so the caption says when the chime would play.
FRAMES = [
    ("demo-1", 2.8, "Four agents at work. docs-pass finished a while ago and waits for a look."),
    ("demo-2", 3.8, "♪ fix-login stops on a permission prompt. It jumps to the top, with what it asked."),
    ("demo-3", 2.6, "Answered, here or with one tap on your phone. Back to work, out of the queue."),
    ("demo-4", 3.2, "♪ checkout-ui finishes and joins the queue, with a summary of what changed."),
]


def svg(rows, caption):
    cols = len(rows[0])
    width = PAD * 2 + cols * CELL_W
    body = TOP + len(rows) * CELL_H + 14
    height = body + 44
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0f}" height="{height:.0f}">',
        f"<style>text{{font-family:{FONT};font-size:14px;white-space:pre}}</style>",
        f'<rect width="100%" height="100%" fill="#191a21"/>',
        f'<rect width="{width}" height="{body}" rx="10" fill="{BG}"/>',
        f'<rect width="{width}" height="30" rx="10" fill="#000" fill-opacity="0.25"/>',
        '<circle cx="18" cy="15" r="6" fill="#ff5f57"/>',
        '<circle cx="38" cy="15" r="6" fill="#febc2e"/>',
        '<circle cx="58" cy="15" r="6" fill="#28c840"/>',
        f'<text x="{width / 2}" y="20" text-anchor="middle" fill="#fff" fill-opacity="0.55">valk · home</text>',
    ]
    for y, row in enumerate(rows):
        top = TOP + y * CELL_H
        # Backgrounds, as runs of one color.
        x = 0
        while x < cols:
            bg = row[x][2]
            end = x
            while end < cols and row[end][2] == bg:
                end += 1
            if bg and bg != BG:
                out.append(f'<rect x="{PAD + x * CELL_W:.1f}" y="{top}" '
                           f'width="{(end - x) * CELL_W + 0.5:.1f}" height="{CELL_H + 0.5}" fill="{bg}"/>')
            x = end
        # Text, as runs of one style, each character placed on its cell.
        x = 0
        while x < cols:
            sym, fg, _, bold = row[x]
            if sym.strip() == "":
                x += 1
                continue
            end = x
            while end < cols and row[end][1] == fg and row[end][3] == bold and row[end][0].strip():
                end += 1
            xs = " ".join(f"{PAD + i * CELL_W:.1f}" for i in range(x, end))
            text = escape("".join(row[i][0] for i in range(x, end)))
            weight = ' font-weight="bold"' if bold else ""
            out.append(f'<text x="{xs}" y="{top + 13.5}" fill="{fg or FG}"{weight}>{text}</text>')
            x = end
    out.append(f'<text x="{width / 2}" y="{body + 28}" text-anchor="middle" '
               f'fill="#b6bdd8" style="font-size:16px">{escape(caption)}</text>')
    out.append("</svg>")
    return "\n".join(out)


def main():
    frames_dir, gif = Path(sys.argv[1]), Path(sys.argv[2])
    with tempfile.TemporaryDirectory() as tmp:
        pngs, delays, colors = [], [], set()
        for name, seconds, caption in FRAMES:
            rows = json.loads((frames_dir / f"{name}.json").read_text())
            colors |= {c for row in rows for cell in row for c in cell[1:3] if c}
            src, png = Path(tmp, f"{name}.svg"), Path(tmp, f"{name}.png")
            src.write_text(svg(rows, caption))
            subprocess.run(["rsvg-convert", src, "-o", png], check=True)
            pngs.append(str(png))
            delays += ["-delay", str(round(seconds * 100)), str(png)]
        # One palette for every frame, without dithering, keeps the theme's
        # colors flat and the same from frame to frame.
        # Small areas such as the accent chips lose their color in a plain
        # quantize, so every color the TUI drew goes in first, then the font's
        # antialiasing fills the rest.
        exact = sorted(colors | {BG, FG, "#191a21", "#b6bdd8", "#ff5f57", "#febc2e", "#28c840"})
        palette = str(Path(tmp, "palette.png"))
        subprocess.run(["magick", *(f"xc:{c}" for c in exact), "+append",
                        "(", *pngs, "-append", "+dither", "-colors", str(256 - len(exact)), "-unique-colors", ")",
                        "+append", "-unique-colors", "-background", BG, "-extent", "256x1", "-depth", "8", palette], check=True)
        # ffmpeg holds each frame for its time and maps it to the palette.
        concat = Path(tmp, "frames.txt")
        concat.write_text("".join(f"file '{png}'\nduration {seconds}\n"
                                  for png, (_, seconds, _) in zip(pngs, FRAMES)) + f"file '{pngs[-1]}'\n")
        subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i", concat,
                        "-i", palette, "-lavfi", "paletteuse=dither=none:diff_mode=rectangle",
                        "-loop", "0", str(gif)], check=True)
    print(f"{gif}: {gif.stat().st_size / 1024:.0f} KB")


if __name__ == "__main__":
    main()
