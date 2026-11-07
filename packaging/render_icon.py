#!/usr/bin/env python3
"""Renders packaging/claude-vision.svg's shapes to PNG with Pillow (same geometry, 64-unit grid).

Usage: python3 packaging/render_icon.py OUT.png [SIZE]
"""
import sys
from PIL import Image, ImageDraw

out = sys.argv[1]
size = int(sys.argv[2]) if len(sys.argv) > 2 else 256
S = 16  # draw at 1024 px, then downscale for smooth edges
img = Image.new("RGBA", (64 * S, 64 * S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
k = lambda *v: [round(x * S) for x in v]

d.rounded_rectangle(k(2, 2, 62, 62), radius=10 * S, fill="#000000", outline="#555555", width=2 * S)
d.rectangle(k(7, 8, 57, 16), fill="#0000AA")
d.rectangle(k(9, 9.5, 11, 14.5), fill="#FFFFFF")
d.rectangle(k(12, 10.5, 27, 13.5), fill="#FFFF55")
d.rectangle(k(44, 10.5, 54, 13.5), fill="#55FF55")
d.rectangle(k(7.5, 19.5, 56.5, 56.5), outline="#AAAAAA", width=round(1.2 * S))
d.rectangle(k(9.6, 21.6, 54.4, 54.4), outline="#AAAAAA", width=round(1.2 * S))
d.ellipse(k(12.8, 25.3, 17.2, 29.7), fill="#55FF55")
d.rectangle(k(19.5, 26, 38.5, 29), fill="#FFFFFF")
d.rectangle(k(41.5, 26, 51, 29), fill="#00AAAA")
d.rectangle(k(13, 33.5, 24, 35.5), fill="#55FFFF")
d.polygon(k(23.5, 31.5, 28, 34.5, 23.5, 37.5), fill="#55FFFF")
d.rectangle(k(32, 33, 51, 36), fill="#AA00AA")
d.polygon(k(13, 41.5, 17.5, 38.5, 17.5, 44.5), fill="#FF55FF")
d.rectangle(k(17, 40.5, 28, 42.5), fill="#FF55FF")
d.rectangle(k(32, 40, 51, 43), fill="#00AA00")
d.rectangle(k(13, 47.5, 36, 50.5), fill="#FFFF55")
d.rectangle(k(36, 47.5, 51, 50.5), fill="#555555")
img.resize((size, size), Image.LANCZOS).save(out)
print(out, size)
