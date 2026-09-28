#!/usr/bin/env python3
"""Draws a stylised PS Vita (original 1000 model proportions) whose screen is
exactly 960x544, for framing screenshots and the trailer in the README.

    python3 tools/vita_frame.py out.png [--screen shot.png]

Prints the screen's top-left corner so videos can be overlaid with ffmpeg.
Everything is drawn from primitives; no product photos are used.
"""
import argparse
import math

from PIL import Image, ImageDraw, ImageFilter

SS = 2  # supersampling factor
W, H = 1800, 900
BODY_W, BODY_H = 1600, 740
SCREEN_W, SCREEN_H = 960, 544


def s(v):
    return int(round(v * SS))


def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(len(a)))


def vgradient(size, top, bottom):
    w, h = size
    g = Image.new("RGBA", (1, h))
    for y in range(h):
        g.putpixel((0, y), lerp(top, bottom, y / max(h - 1, 1)))
    return g.resize((w, h))


def rounded_mask(size, radius):
    m = Image.new("L", size, 0)
    ImageDraw.Draw(m).rounded_rectangle([0, 0, size[0] - 1, size[1] - 1], radius=radius, fill=255)
    return m


def draw_button(img, cx, cy, r, base=(30, 31, 36, 255)):
    d = ImageDraw.Draw(img)
    d.ellipse([s(cx - r - 3), s(cy - r - 3), s(cx + r + 3), s(cy + r + 3)], fill=(8, 8, 10, 255))
    cap = vgradient((s(2 * r), s(2 * r)), lerp(base, (255, 255, 255, 255), 0.10), lerp(base, (0, 0, 0, 255), 0.25))
    mask = Image.new("L", cap.size, 0)
    ImageDraw.Draw(mask).ellipse([0, 0, cap.size[0] - 1, cap.size[1] - 1], fill=255)
    img.paste(cap, (s(cx - r), s(cy - r)), mask)


def draw_stick(img, cx, cy):
    d = ImageDraw.Draw(img)
    d.ellipse([s(cx - 60), s(cy - 60), s(cx + 60), s(cy + 60)], fill=(6, 6, 8, 255))
    draw_button(img, cx, cy, 47, base=(44, 45, 51, 255))
    d = ImageDraw.Draw(img)
    # Concave top: a darker inner disc with a soft rim.
    d.ellipse([s(cx - 32), s(cy - 32), s(cx + 32), s(cy + 32)], fill=(30, 31, 36, 255))
    d.ellipse([s(cx - 32), s(cy - 32), s(cx + 32), s(cy + 32)], outline=(58, 59, 66, 255), width=s(2))


def draw_symbol(img, kind, cx, cy, color):
    d = ImageDraw.Draw(img)
    w = s(4.5)
    k = 13
    if kind == "triangle":
        pts = [(cx, cy - k - 1), (cx + k + 1, cy + k - 3), (cx - k - 1, cy + k - 3)]
        d.line([(s(x), s(y)) for x, y in pts + [pts[0]]], fill=color, width=w, joint="curve")
    elif kind == "circle":
        d.ellipse([s(cx - k), s(cy - k), s(cx + k), s(cy + k)], outline=color, width=w)
    elif kind == "cross":
        d.line([s(cx - k), s(cy - k), s(cx + k), s(cy + k)], fill=color, width=w)
        d.line([s(cx + k), s(cy - k), s(cx - k), s(cy + k)], fill=color, width=w)
    else:
        d.rectangle([s(cx - k + 1), s(cy - k + 1), s(cx + k - 1), s(cy + k - 1)], outline=color, width=w)


def draw_dpad(img, cx, cy):
    d = ImageDraw.Draw(img)
    d.ellipse([s(cx - 86), s(cy - 86), s(cx + 86), s(cy + 86)], fill=(20, 21, 24, 255))
    arm_l, arm_w = 70, 23
    for dx, dy in [(0, -1), (0, 1), (-1, 0), (1, 0)]:
        x0 = cx + (dx * arm_l if dx else -arm_w) - (0 if dx >= 0 else 0)
        rect = (
            [cx - arm_w, cy - arm_l, cx + arm_w, cy] if dy == -1 else
            [cx - arm_w, cy, cx + arm_w, cy + arm_l] if dy == 1 else
            [cx - arm_l, cy - arm_w, cx, cy + arm_w] if dx == -1 else
            [cx, cy - arm_w, cx + arm_l, cy + arm_w]
        )
        d.rounded_rectangle([s(v) for v in rect], radius=s(8), fill=(40, 41, 47, 255))
        # Direction arrows.
        ax, ay = cx + dx * 48, cy + dy * 48
        t = 8
        if dx == 0:
            pts = [(ax, ay + dy * t), (ax - t, ay - dy * t), (ax + t, ay - dy * t)]
        else:
            pts = [(ax + dx * t, ay), (ax - dx * t, ay - t), (ax - dx * t, ay + t)]
        d.polygon([(s(x), s(y)) for x, y in pts], fill=(20, 21, 24, 255))
    d.ellipse([s(cx - 16), s(cy - 16), s(cx + 16), s(cy + 16)], fill=(34, 35, 40, 255))


def draw_pill(img, cx, cy, label):
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([s(cx - 22), s(cy - 8), s(cx + 22), s(cy + 8)], radius=s(8), fill=(8, 8, 10, 255))
    d.rounded_rectangle([s(cx - 19), s(cy - 5), s(cx + 19), s(cy + 5)], radius=s(5), fill=(46, 47, 53, 255))


def draw_grille(img, cx, cy):
    d = ImageDraw.Draw(img)
    for row in range(3):
        for col in range(6):
            x = cx + (col - 2.5) * 11
            y = cy + (row - 1) * 11
            d.ellipse([s(x - 2.4), s(y - 2.4), s(x + 2.4), s(y + 2.4)], fill=(10, 10, 12, 255))


def build(screen_img=None):
    img = Image.new("RGBA", (s(W), s(H)), (0, 0, 0, 0))

    bx, by = (W - BODY_W) / 2, (H - BODY_H) / 2
    radius = 300

    # Drop shadow.
    shadow = Image.new("L", img.size, 0)
    ImageDraw.Draw(shadow).rounded_rectangle(
        [s(bx + 20), s(by + 40), s(bx + BODY_W - 20), s(by + BODY_H + 24)], radius=s(radius), fill=150)
    shadow = shadow.filter(ImageFilter.GaussianBlur(s(26)))
    img.paste(Image.new("RGBA", img.size, (0, 0, 0, 255)), (0, 0), shadow)

    # Body with a glossy top-to-bottom gradient and a thin light rim.
    body = vgradient((s(BODY_W), s(BODY_H)), (48, 49, 55, 255), (18, 19, 22, 255))
    img.paste(body, (s(bx), s(by)), rounded_mask(body.size, s(radius)))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([s(bx), s(by), s(bx + BODY_W), s(by + BODY_H)], radius=s(radius),
                        outline=(78, 80, 88, 255), width=s(2))
    inner = vgradient((s(BODY_W - 16), s(BODY_H - 16)), (34, 35, 40, 255), (14, 15, 18, 255))
    img.paste(inner, (s(bx + 8), s(by + 8)), rounded_mask(inner.size, s(radius - 8)))

    # Screen bezel and screen.
    sx, sy = int(bx + (BODY_W - SCREEN_W) / 2), int(by + (BODY_H - SCREEN_H) / 2)
    d.rounded_rectangle([s(sx - 26), s(sy - 22), s(sx + SCREEN_W + 26), s(sy + SCREEN_H + 22)],
                        radius=s(22), fill=(6, 6, 8, 255))
    d.rounded_rectangle([s(sx - 26), s(sy - 22), s(sx + SCREEN_W + 26), s(sy + SCREEN_H + 22)],
                        radius=s(22), outline=(40, 41, 46, 255), width=s(2))
    d.rectangle([s(sx), s(sy), s(sx + SCREEN_W), s(sy + SCREEN_H)], fill=(0, 0, 0, 255))
    # Front camera.
    d.ellipse([s(sx + SCREEN_W + 70), s(by + 70), s(sx + SCREEN_W + 84), s(by + 84)], fill=(4, 4, 6, 255))
    d.ellipse([s(sx + SCREEN_W + 74), s(by + 74), s(sx + SCREEN_W + 80), s(by + 80)], fill=(30, 34, 52, 255))

    # Left side: D-pad, left stick, PS button, speaker.
    lcx = bx + (sx - bx) / 2
    draw_dpad(img, lcx + 6, by + 250)
    draw_stick(img, lcx + 70, by + 488)
    draw_button(img, lcx - 18, by + 628, 17, base=(40, 41, 47, 255))
    draw_grille(img, lcx - 70, by + 560)

    # Right side: face buttons, right stick, Start/Select, speaker.
    rcx = sx + SCREEN_W + (bx + BODY_W - sx - SCREEN_W) / 2
    fcx, fcy, off = rcx - 6, by + 250, 70
    for kind, dx, dy, color in [
        ("triangle", 0, -1, (66, 217, 180, 255)),
        ("circle", 1, 0, (255, 111, 134, 255)),
        ("cross", 0, 1, (127, 168, 255, 255)),
        ("square", -1, 0, (242, 143, 217, 255)),
    ]:
        draw_button(img, fcx + dx * off, fcy + dy * off, 33)
        draw_symbol(img, kind, fcx + dx * off, fcy + dy * off, color)
    draw_stick(img, rcx - 70, by + 488)
    draw_pill(img, rcx - 10, by + 628, "SELECT")
    draw_pill(img, rcx + 50, by + 628, "START")
    draw_grille(img, rcx + 70, by + 560)

    img = img.resize((W, H), Image.LANCZOS)
    if screen_img is not None:
        img.paste(screen_img.convert("RGBA").resize((SCREEN_W, SCREEN_H)), (sx, sy))
    return img, (sx, sy)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--screen")
    ap.add_argument("--background", default="#0d1117")
    args = ap.parse_args()
    screen = Image.open(args.screen) if args.screen else None
    frame, (sx, sy) = build(screen)
    if args.background != "none":
        bg = Image.new("RGBA", frame.size, args.background)
        bg.alpha_composite(frame)
        frame = bg
    frame.save(args.out)
    print(f"{sx} {sy}")


if __name__ == "__main__":
    main()
