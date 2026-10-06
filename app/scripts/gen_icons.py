#!/usr/bin/env python3
"""Generates the app and tray icons for gdrive-app.

Run from anywhere:  python3 app/scripts/gen_icons.py
Writes into app/src-tauri/icons/ (plus app/src/assets/logo.png). Requires Pillow.

Everything is drawn at 8x and downsampled, which gives clean anti-aliased edges
without needing an SVG renderer.
"""

from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

OUT = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"
UI = Path(__file__).resolve().parent.parent / "src" / "assets"
SS = 8  # supersampling factor

BLUE = (66, 133, 244, 255)
BLUE_DARK = (40, 98, 214, 255)
GREY = (154, 160, 166, 255)
GREY_DARK = (110, 116, 122, 255)
RED = (234, 67, 53, 255)
AMBER = (249, 171, 0, 255)
WHITE = (255, 255, 255, 255)


def cloud_mask(size, box):
    """A flat cloud silhouette fitted into `box` (x0, y0, x1, y1) on a `size` canvas."""
    x0, y0, x1, y1 = box
    w, h = x1 - x0, y1 - y0
    m = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(m)

    def circle(cx, cy, r):
        px, py, pr = x0 + cx * w, y0 + cy * h, r * w
        d.ellipse((px - pr, py - pr, px + pr, py + pr), fill=255)

    # Coordinates are fractions of the box; radii are fractions of its width.
    base_top = 0.56
    d.rounded_rectangle((x0 + 0.04 * w, y0 + base_top * h, x1 - 0.04 * w, y1),
                        radius=(y1 - (y0 + base_top * h)) / 2, fill=255)
    circle(0.25, 0.66, 0.19)   # small left puff
    circle(0.47, 0.47, 0.265)  # big middle puff
    circle(0.72, 0.58, 0.215)  # right puff
    return m


def draw_check(d, cx, cy, s, width, fill):
    pts = [(cx - 0.42 * s, cy + 0.02 * s), (cx - 0.12 * s, cy + 0.32 * s), (cx + 0.45 * s, cy - 0.30 * s)]
    d.line(pts, fill=fill, width=width, joint="curve")
    for p in (pts[0], pts[-1]):
        d.ellipse((p[0] - width / 2, p[1] - width / 2, p[0] + width / 2, p[1] + width / 2), fill=fill)


def draw_sync(img, cx, cy, r, width, fill):
    """Two circular arrows chasing each other."""
    layer = Image.new("RGBA", img.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    bbox = (cx - r, cy - r, cx + r, cy + r)
    import math
    for start in (200, 20):
        end = start + 125
        d.arc(bbox, start=start, end=end, fill=fill, width=width)
        # Round tail cap.
        a = math.radians(start)
        tx, ty = cx + (r - width / 2) * math.cos(a), cy + (r - width / 2) * math.sin(a)
        d.ellipse((tx - width / 2, ty - width / 2, tx + width / 2, ty + width / 2), fill=fill)
        # Arrow head at the end of the arc, pointing along the direction of travel.
        a = math.radians(end)
        mx, my = cx + (r - width / 2) * math.cos(a), cy + (r - width / 2) * math.sin(a)
        tang = (-math.sin(a), math.cos(a))
        norm = (math.cos(a), math.sin(a))
        hs = width * 1.25
        tip = (mx + tang[0] * hs * 1.1, my + tang[1] * hs * 1.1)
        b1 = (mx + norm[0] * hs, my + norm[1] * hs)
        b2 = (mx - norm[0] * hs, my - norm[1] * hs)
        d.polygon([tip, b1, b2], fill=fill)
    img.alpha_composite(layer)


def draw_pause(d, cx, cy, s, fill):
    bw, bh, gap = 0.20 * s, 0.62 * s, 0.17 * s
    for x in (cx - gap - bw, cx + gap):
        d.rounded_rectangle((x, cy - bh / 2, x + bw, cy + bh / 2), radius=bw / 2, fill=fill)


def draw_exclaim(d, cx, cy, s, fill):
    w = 0.16 * s
    d.rounded_rectangle((cx - w / 2, cy - 0.40 * s, cx + w / 2, cy + 0.12 * s), radius=w / 2, fill=fill)
    d.ellipse((cx - w * 0.6, cy + 0.24 * s, cx + w * 0.6, cy + 0.24 * s + w * 1.2), fill=fill)


def knockout(img, glyph_mask):
    """Makes the glyph area transparent (a cut-out), so it reads on any panel colour."""
    a = img.getchannel("A")
    a = ImageChops.subtract(a, glyph_mask)
    img.putalpha(a)


def tray_icon(state, px=64):
    S = px * SS
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    box = (S * 0.03, S * 0.17, S * 0.97, S * 0.83)
    color = {
        "idle": BLUE, "syncing": BLUE, "paused": GREY_DARK,
        "error": BLUE, "offline": GREY, "signedout": GREY,
    }[state]
    mask = cloud_mask(S, box)
    body = Image.new("RGBA", (S, S), color)
    img.paste(body, (0, 0), mask)

    # Glyph centred in the cloud's main body.
    gx, gy, gs = S * 0.50, S * 0.58, S * 0.36
    gmask = Image.new("L", (S, S), 0)
    gd = ImageDraw.Draw(gmask)
    if state == "idle":
        draw_check(gd, gx, gy, gs, int(S * 0.075), 255)
    elif state == "syncing":
        tmp = Image.new("RGBA", (S, S), (0, 0, 0, 0))
        draw_sync(tmp, gx, gy, gs * 0.48, int(S * 0.062), WHITE)
        gmask = tmp.getchannel("A")
    elif state == "paused":
        draw_pause(gd, gx, gy, gs, 255)
    elif state == "offline":
        draw_exclaim(gd, gx, gy, gs, 255)
    if state != "signedout" and state != "error":
        knockout(img, gmask)

    if state == "error":
        # Red badge with "!" in the lower-right corner, separated by a cut-out ring.
        bx, by, br = S * 0.73, S * 0.66, S * 0.25
        ring = Image.new("L", (S, S), 0)
        ImageDraw.Draw(ring).ellipse((bx - br - S * 0.05, by - br - S * 0.05,
                                      bx + br + S * 0.05, by + br + S * 0.05), fill=255)
        knockout(img, ring)
        d = ImageDraw.Draw(img)
        d.ellipse((bx - br, by - br, bx + br, by + br), fill=RED)
        draw_exclaim(d, bx, by, br * 1.55, WHITE)
    return img.resize((px, px), Image.LANCZOS)


def app_icon(px):
    S = 1024
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    # Rounded-square tile with a vertical gradient.
    grad = Image.new("RGBA", (S, S))
    top, bot = (92, 157, 255), (37, 99, 220)
    gd = ImageDraw.Draw(grad)
    for y in range(S):
        t = y / (S - 1)
        gd.line([(0, y), (S, y)], fill=tuple(int(top[i] + (bot[i] - top[i]) * t) for i in range(3)) + (255,))
    pad = int(S * 0.06)
    tile = Image.new("L", (S, S), 0)
    ImageDraw.Draw(tile).rounded_rectangle((pad, pad, S - pad, S - pad), radius=int(S * 0.22), fill=255)
    # Soft drop shadow under the tile.
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    sh = tile.filter(ImageFilter.GaussianBlur(S * 0.02)).point(lambda v: int(v * 0.35))
    shadow.putalpha(sh)
    img.alpha_composite(shadow, (0, int(S * 0.012)))
    img.paste(grad, (0, 0), tile)

    box = (S * 0.20, S * 0.27, S * 0.80, S * 0.73)
    cmask = cloud_mask(S, box)
    cshadow = Image.new("RGBA", (S, S), (10, 40, 120, 0))
    cshadow.putalpha(cmask.filter(ImageFilter.GaussianBlur(S * 0.018)).point(lambda v: int(v * 0.30)))
    img.alpha_composite(cshadow, (0, int(S * 0.018)))
    img.paste(Image.new("RGBA", (S, S), WHITE), (0, 0), cmask)
    # Small blue check inside the cloud.
    d = ImageDraw.Draw(img)
    draw_check(d, S * 0.50, S * 0.585, S * 0.17, int(S * 0.045), BLUE_DARK)
    return img.resize((px, px), Image.LANCZOS)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for name, px in (("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256), ("icon.png", 512)):
        app_icon(px).save(OUT / name)
    app_icon(192).save(UI / "logo.png")  # onboarding hero image
    for state in ("idle", "syncing", "paused", "error", "offline", "signedout"):
        tray_icon(state).save(OUT / f"tray-{state}.png")
    print(f"icons written to {OUT}")


if __name__ == "__main__":
    main()
