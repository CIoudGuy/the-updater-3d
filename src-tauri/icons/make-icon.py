"""Builds the app icon from the logo artwork.

The logo (gold refresh arrow over an off-white box, on a transparent background) is centred on a
dark rounded tile with even padding, drawn at 4x and scaled down for clean edges.

    python -I make-icon.py <logo.png> <out-1024.png>
Then: npx tauri icon <out-1024.png> -o <folder> and copy 32x32.png, 128x128.png,
128x128@2x.png, icon.png and icon.ico into this folder.
"""
import sys
from PIL import Image, ImageDraw

SRC, OUT = sys.argv[1], sys.argv[2]
S = 4096                      # work size
INSET = 0.0                   # the tile fills the icon
RADIUS = 0.22                 # corner radius, share of the tile size
LOGO = 0.80                   # longest side of the logo, share of the tile size
TILE = (30, 30, 32, 255)      # one solid dark grey

logo = Image.open(SRC).convert("RGBA")
logo = logo.crop(logo.getbbox())

tile = int(S * (1 - 2 * INSET))
off = (S - tile) // 2

# The tile: a solid rounded square.
shade = Image.new("RGBA", (tile, tile), TILE)
mask = Image.new("L", (tile, tile), 0)
ImageDraw.Draw(mask).rounded_rectangle((0, 0, tile - 1, tile - 1), radius=int(tile * RADIUS), fill=255)
canvas = Image.new("RGBA", (S, S), (0, 0, 0, 0))
canvas.paste(shade, (off, off), mask)

# The logo, centred on the tile.
scale = tile * LOGO / max(logo.size)
logo = logo.resize((round(logo.width * scale), round(logo.height * scale)), Image.LANCZOS)
canvas.alpha_composite(logo, ((S - logo.width) // 2, (S - logo.height) // 2))

canvas.resize((1024, 1024), Image.LANCZOS).save(OUT)
