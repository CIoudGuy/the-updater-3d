"""Builds the app icon from the source artwork.

The source PNG (cube and gold brush stroke on a rough dark tile) is matted: every pixel is
split into tile colour plus one of the two ink colours, which gives clean soft edges with no
dark fringe. The ink is then placed on a freshly drawn neutral grey rounded tile.

    python -I make-icon.py <source.png> <out-1024.png>
Then: npx tauri icon <out-1024.png>
"""
import sys
import numpy as np
from PIL import Image, ImageDraw

SRC, OUT = sys.argv[1], sys.argv[2]
TILE = np.array([28, 28, 28], dtype=np.float32)      # tile colour in the source
INKS = np.array([[238, 238, 238], [222, 182, 104]], dtype=np.float32)  # off-white, ochre
BG = (30, 30, 30, 255)                               # our tile: neutral #1E1E1E
S = 4096                                             # work size, downscaled at the end

src = np.asarray(Image.open(SRC).convert("RGBA")).astype(np.float32)
rgb, a0 = src[..., :3], src[..., 3] / 255.0

# Alpha of each ink: projection of (pixel - tile) onto (ink - tile).
best_alpha = np.zeros(a0.shape, np.float32)
best_ink = np.zeros(a0.shape, np.int32)
for k, ink in enumerate(INKS):
    d = ink - TILE
    t = ((rgb - TILE) @ d) / float(d @ d)
    # Penalise pixels whose colour is far from the line tile -> ink (wrong ink).
    proj = TILE + t[..., None] * d
    off = np.linalg.norm(rgb - proj, axis=-1)
    score = np.clip(t, 0, 1) * (off < 60)
    take = score > best_alpha
    best_alpha[take] = score[take]
    best_ink[take] = k
alpha = np.clip((best_alpha - 0.08) / 0.84, 0, 1)
# Only keep ink well inside the old tile: its soft outer edge is not artwork.
from PIL import ImageFilter
inside = Image.fromarray(((a0 >= 0.99) * 255).astype(np.uint8)).filter(ImageFilter.MinFilter(41))
alpha *= np.asarray(inside, np.float32) / 255.0
ink_rgb = INKS[best_ink]

art = np.dstack([ink_rgb, alpha * 255]).astype(np.uint8)
art = Image.fromarray(art, "RGBA")
bbox = art.getchannel("A").point(lambda v: 255 if v > 128 else 0).getbbox()
art = art.crop(bbox)

# Our tile: rounded square with a little margin, like Windows app icons.
canvas = Image.new("RGBA", (S, S), (0, 0, 0, 0))
pad = int(S * 0.055)
ImageDraw.Draw(canvas).rounded_rectangle([pad, pad, S - pad, S - pad], radius=int(S * 0.215), fill=BG)

# Artwork fills about 66% of the tile, optically centred (a touch high, the stroke is light).
inner = S - 2 * pad
scale = min(inner * 0.66 / art.width, inner * 0.70 / art.height)
art = art.resize((round(art.width * scale), round(art.height * scale)), Image.LANCZOS)
x = (S - art.width) // 2
y = (S - art.height) // 2 - int(S * 0.008)
canvas.alpha_composite(art, (x, y))

canvas.resize((1024, 1024), Image.LANCZOS).save(OUT)
print("saved", OUT, "art bbox", bbox)

# Small sizes (16 to 32 px): bolder strokes and a bigger cube so it still reads in the tray.
small = Image.new("RGBA", (S, S), (0, 0, 0, 0))
ImageDraw.Draw(small).rounded_rectangle([pad, pad, S - pad, S - pad], radius=int(S * 0.215), fill=BG)
bold = art.copy()
a = bold.getchannel("A").filter(ImageFilter.MaxFilter(max(3, (art.width // 60) | 1)))
bold.putalpha(a)
scale2 = min(inner * 0.80 / bold.width, inner * 0.82 / bold.height)
bold = bold.resize((round(bold.width * scale2), round(bold.height * scale2)), Image.LANCZOS)
small.alpha_composite(bold, ((S - bold.width) // 2, (S - bold.height) // 2))
small_path = OUT.replace(".png", "-small.png")
small.resize((256, 256), Image.LANCZOS).save(small_path)

# Windows icon: small art for 16-32 px, full art above.
big = canvas.resize((256, 256), Image.LANCZOS)
sm = Image.open(small_path)
frames = [sm.resize((n, n), Image.LANCZOS) for n in (16, 20, 24, 32)] + [big.resize((n, n), Image.LANCZOS) for n in (40, 48, 64, 128, 256)]
ico = OUT.replace("app-icon-1024.png", "icon.ico")
frames[-1].save(ico, format="ICO", sizes=[f.size for f in frames], append_images=frames[:-1])
print("saved", small_path, ico)
