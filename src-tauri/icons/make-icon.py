#!/usr/bin/env python3
"""
Generate pontifex's application icon.

Kept in the repo because the icon is derived, not drawn by hand: it is built
from the same palette the UI uses (`src/index.css`), so a theme change can be
carried into the icon by editing the values here rather than by opening an
image editor and matching colours by eye.

    python3 src-tauri/icons/make-icon.py        # writes icon-source.png
    cargo tauri icon src-tauri/icons/icon-source.png

The mark is a pair of braces around three dots: a schema (braces, as in JSON)
containing a stream of events (dots). Both halves of what the app is for, and
still legible at 32px where anything more detailed turns to mud.
"""

import math
import os

from PIL import Image, ImageDraw

# --- palette -----------------------------------------------------------------
# oklch values copied from the `@theme` block in src/index.css, converted here
# so the two cannot drift silently.


def _gamma(x: float) -> float:
    return 12.92 * x if x <= 0.0031308 else 1.055 * (x ** (1 / 2.4)) - 0.055


def oklch(lightness: float, chroma: float, hue_deg: float) -> tuple[int, int, int]:
    """oklch -> sRGB, matching how the browser resolves the same declaration."""
    hue = math.radians(hue_deg)
    a, b = chroma * math.cos(hue), chroma * math.sin(hue)
    l_ = lightness + 0.3963377774 * a + 0.2158037573 * b
    m_ = lightness - 0.1055613458 * a - 0.0638541728 * b
    s_ = lightness - 0.0894841775 * a - 1.2914855480 * b
    l, m, s = l_**3, m_**3, s_**3
    linear = (
        +4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    )
    return tuple(  # type: ignore[return-value]
        max(0, min(255, round(_gamma(max(0.0, min(1.0, v))) * 255))) for v in linear
    )


SURFACE_TOP = oklch(0.26, 0.014, 265)  # --color-surface-2
SURFACE_BOTTOM = oklch(0.16, 0.012, 265)  # a shade under --color-surface-0
ACCENT = oklch(0.78, 0.15, 75)  # --color-accent
INK = oklch(0.94, 0.005, 265)  # --color-ink
EDGE = oklch(0.34, 0.016, 265)  # --color-edge

# --- geometry ----------------------------------------------------------------

SIZE = 1024
SS = 4  # supersample factor; edges are downsampled to smooth them
MARGIN = 44
RADIUS = 210

BRACE_TOP, BRACE_BOTTOM = 286, 738
BRACE_STROKE = 58
LEFT_TIP, LEFT_END = 246, 356
# Spaced so the three stay distinct at 128px; any tighter and they blur into a
# single rounded bar once downsampled.
DOT_RADIUS = 33
DOT_XS = (420, 512, 604)


def bezier(p0, p1, p2, p3, steps=160):
    """Cubic bezier as points, for stroking with a rounded joint."""
    out = []
    for i in range(steps + 1):
        t = i / steps
        u = 1 - t
        out.append(
            (
                u**3 * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t**3 * p3[0],
                u**3 * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t**3 * p3[1],
            )
        )
    return out


def brace_points(tip_x: float, end_x: float, top: float, bottom: float):
    """A curly brace, tip pointing toward `tip_x`."""
    height = bottom - top
    spine_x = tip_x + (end_x - tip_x) * 0.46
    mid = top + height / 2

    def y(fraction: float) -> float:
        return top + height * fraction

    points = []
    points += bezier(
        (end_x, y(0.0)),
        (spine_x + (tip_x - spine_x) * 0.15, y(0.0)),
        (spine_x, y(0.05)),
        (spine_x, y(0.30)),
    )
    points += bezier(
        (spine_x, y(0.30)),
        (spine_x, y(0.42)),
        (spine_x + (tip_x - spine_x) * 0.55, y(0.44)),
        (tip_x, mid),
    )[1:]
    points += bezier(
        (tip_x, mid),
        (spine_x + (tip_x - spine_x) * 0.55, y(0.56)),
        (spine_x, y(0.58)),
        (spine_x, y(0.70)),
    )[1:]
    points += bezier(
        (spine_x, y(0.70)),
        (spine_x, y(0.95)),
        (spine_x + (tip_x - spine_x) * 0.15, y(1.0)),
        (end_x, y(1.0)),
    )[1:]
    return points


def resample(points, spacing):
    """Points along a path at most `spacing` apart, so stamped discs overlap."""
    out = [points[0]]
    for (x0, y0), (x1, y1) in zip(points, points[1:]):
        distance = math.hypot(x1 - x0, y1 - y0)
        steps = max(1, int(distance / spacing))
        for i in range(1, steps + 1):
            t = i / steps
            out.append((x0 + (x1 - x0) * t, y0 + (y1 - y0) * t))
    return out


def render() -> Image.Image:
    w = SIZE * SS
    canvas = Image.new("RGBA", (w, w), (0, 0, 0, 0))

    # Background: a vertical gradient inside a rounded square. macOS draws app
    # icons unmasked, so the rounded shape has to be part of the image.
    gradient = Image.new("RGB", (1, SIZE), SURFACE_BOTTOM)
    for y in range(SIZE):
        t = y / (SIZE - 1)
        gradient.putpixel(
            (0, y),
            tuple(  # type: ignore[arg-type]
                round(SURFACE_TOP[i] + (SURFACE_BOTTOM[i] - SURFACE_TOP[i]) * t)
                for i in range(3)
            ),
        )
    gradient = gradient.resize((w, w), Image.Resampling.BICUBIC).convert("RGBA")

    mask = Image.new("L", (w, w), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [MARGIN * SS, MARGIN * SS, (SIZE - MARGIN) * SS, (SIZE - MARGIN) * SS],
        radius=RADIUS * SS,
        fill=255,
    )
    canvas.paste(gradient, (0, 0), mask)

    # A hairline edge stops the tile dissolving into a dark dock background.
    ImageDraw.Draw(canvas).rounded_rectangle(
        [MARGIN * SS, MARGIN * SS, (SIZE - MARGIN) * SS, (SIZE - MARGIN) * SS],
        radius=RADIUS * SS,
        outline=(*EDGE, 190),
        width=3 * SS,
    )

    draw = ImageDraw.Draw(canvas)

    # Stroked by stamping a disc along the path rather than with
    # `ImageDraw.line(joint="curve")`, which mitres each segment separately and
    # leaves visible spikes wherever the curvature is tight.
    stroke = Image.new("L", (w, w), 0)
    stamp = ImageDraw.Draw(stroke)
    radius = BRACE_STROKE * SS / 2
    for points in (
        brace_points(LEFT_TIP, LEFT_END, BRACE_TOP, BRACE_BOTTOM),
        brace_points(SIZE - LEFT_TIP, SIZE - LEFT_END, BRACE_TOP, BRACE_BOTTOM),
    ):
        for x, y in resample(points, radius / 3):
            stamp.ellipse(
                [x * SS - radius, y * SS - radius, x * SS + radius, y * SS + radius],
                fill=255,
            )
    canvas.paste(Image.new("RGBA", (w, w), (*ACCENT, 255)), (0, 0), stroke)

    for x in DOT_XS:
        r = DOT_RADIUS * SS
        draw.ellipse(
            [x * SS - r, SIZE / 2 * SS - r, x * SS + r, SIZE / 2 * SS + r],
            fill=(*INK, 255),
        )

    return canvas.resize((SIZE, SIZE), Image.Resampling.LANCZOS)


if __name__ == "__main__":
    target = os.path.join(os.path.dirname(os.path.abspath(__file__)), "icon-source.png")
    render().save(target)
    print(f"wrote {target}")
