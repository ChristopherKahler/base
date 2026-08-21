#!/usr/bin/env python3
"""Generate the README's branded SVG assets: docs/splash.svg and docs/demo.svg.

Styling is traced from the basemode site brand tokens (light-only, paper +
ink + one blue + one terracotta mark). The wordmark is outlined from Literata
at weight 600 so the splash renders identically on every machine — no font
loading in SVG-as-<img> on GitHub.

Usage:
    python3 scripts/readme-assets.py --font /path/to/Literata[opsz,wght].ttf

Literata: https://github.com/google/fonts/tree/main/ofl/literata (OFL).
"""

import argparse
from pathlib import Path

# brand tokens — traced from the basemode site (src/styles/tokens.css)
PAPER = "#F5F4F1"
RAISED = "#FBFAF8"
INK = "#12263A"
TEXT2 = "#2F4A63"
TEXT3 = "#71879B"
LINE = "#DCDBD5"
LINE2 = "#E7E6E1"
BLUE = "#0B63D6"
BLUE300 = "#8FB8F0"
BLUE100 = "#E7F0FD"
MARK = "#C2551F"  # terracotta: once per screen, nowhere else

MONO = "'IBM Plex Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"
CHAR_W = 6.6  # IBM Plex Mono advance at 11px (600/1000 em)


# ---------------------------------------------------------------- wordmark

def wordmark_paths(font_path: str, text: str, size: float, tracking_em: float = -0.02):
    """Outline `text` from the font at `size` px. Returns (paths, width) where
    paths is a list of (svg_path_d, advance_x_offset) per glyph."""
    from fontTools.ttLib import TTFont
    from fontTools.varLib.instancer import instantiateVariableFont
    from fontTools.pens.svgPathPen import SVGPathPen

    font = TTFont(font_path)
    if "fvar" in font:
        instantiateVariableFont(font, {"wght": 600, "opsz": 24}, inplace=True)
    upem = font["head"].unitsPerEm
    scale = size / upem
    cmap = font.getBestCmap()
    glyph_set = font.getGlyphSet()
    hmtx = font["hmtx"]

    paths, x = [], 0.0
    for ch in text:
        gname = cmap[ord(ch)]
        pen = SVGPathPen(glyph_set)
        glyph_set[gname].draw(pen)
        d = pen.getCommands()
        paths.append((d, x))
        x += hmtx[gname][0] * scale + tracking_em * size
    return paths, x - tracking_em * size


def build_splash(font_path: str, wordmark: str = "basemode") -> str:
    W, H = 1200, 300
    size = 76
    paths, width = wordmark_paths(font_path, wordmark + ".", size)
    ox = (W - width) / 2
    oy = 158  # baseline

    glyphs = []
    for i, (d, dx) in enumerate(paths):
        fill = MARK if i == len(paths) - 1 else INK
        # font units are y-up; flip around the baseline
        glyphs.append(
            f'<path transform="translate({ox + dx:.1f},{oy}) scale({size / 1000:.6f},-{size / 1000:.6f})" '
            f'd="{d}" fill="{fill}"/>'
        )

    eyebrow = "THE MEMORY YOUR AGENTS WERE SUPPOSED TO COME WITH"

    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" role="img" aria-label="basemode — the memory your agents were supposed to come with">
<defs>
<radialGradient id="bloom" cx="0.5" cy="0.38" r="0.75">
<stop offset="0" stop-color="#FFFFFF" stop-opacity="0.9"/>
<stop offset="0.55" stop-color="{RAISED}" stop-opacity="0.4"/>
<stop offset="1" stop-color="{PAPER}" stop-opacity="0"/>
</radialGradient>
<filter id="grain"><feTurbulence type="fractalNoise" baseFrequency="0.82" numOctaves="3" stitchTiles="stitch"/><feColorMatrix type="matrix" values="0 0 0 0 0.07 0 0 0 0 0.15 0 0 0 0 0.23 0 0 0 0.05 0"/></filter>
</defs>
<rect width="{W}" height="{H}" fill="{PAPER}"/>
<rect width="{W}" height="{H}" fill="url(#bloom)"/>
<rect width="{W}" height="{H}" filter="url(#grain)"/>
<rect x="0.5" y="0.5" width="{W - 1}" height="{H - 1}" fill="none" stroke="{LINE}"/>
{"".join(glyphs)}
<text x="{W / 2}" y="212" text-anchor="middle" font-family={MONO!r} font-size="12.5" font-weight="500" letter-spacing="1.75" fill="{TEXT3}">{eyebrow}</text>
</svg>'''


# ---------------------------------------------------------------- demo

def typed(text: str, x: float, y: float, t0: float, cps: float, dur: float,
          fill: str, size: float = 11.5, char_w: float | None = None) -> tuple[str, float]:
    """Per-character SMIL typing. Returns (svg, finish_time)."""
    cw = char_w if char_w else size * 0.6
    out = []
    for i, ch in enumerate(text):
        if ch == " ":
            continue
        show = t0 + i / cps
        k1, k2 = show / dur, min((show + 0.08) / dur, 0.955)
        c = ch.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;")
        out.append(
            f'<text x="{x + i * cw:.1f}" y="{y}" font-size="{size}" fill="{fill}" opacity="0">{c}'
            f'<animate attributeName="opacity" values="0;0;1;1;0" keyTimes="0;{k1:.4f};{k2:.4f};0.96;1" dur="{dur}s" repeatCount="indefinite"/></text>'
        )
    return "".join(out), t0 + len(text) / cps


def fade(inner: str, t0: float, dur: float) -> str:
    k = t0 / dur
    return (f'<g opacity="0">{inner}'
            f'<animate attributeName="opacity" values="0;0;1;1;0" keyTimes="0;{k:.4f};{min(k + 0.03, 0.94):.4f};0.96;1" dur="{dur}s" repeatCount="indefinite"/></g>')


def build_demo() -> str:
    W, H, DUR = 900, 380, 14.0
    parts = []

    # ---- card
    parts.append(f'''<defs>
<filter id="shadow" x="-6%" y="-6%" width="112%" height="115%"><feDropShadow dx="0" dy="8" stdDeviation="10" flood-color="{BLUE}" flood-opacity="0.14"/></filter>
<linearGradient id="card" x1="0.1" y1="0" x2="0.35" y2="1"><stop offset="0" stop-color="{RAISED}"/><stop offset="1" stop-color="#EFEEEA"/></linearGradient>
</defs>
<rect width="{W}" height="{H}" fill="{PAPER}"/>
<rect x="14" y="10" width="{W - 28}" height="{H - 24}" rx="16" fill="url(#card)" stroke="{LINE}" filter="url(#shadow)"/>
<line x1="510" y1="52" x2="510" y2="{H - 40}" stroke="{LINE2}"/>
<line x1="14" y1="52" x2="{W - 14}" y2="52" stroke="{LINE2}"/>''')

    # ---- card furniture: eyebrow left, live chip right
    parts.append(f'<text x="38" y="37" font-family={MONO!r} font-size="10.5" letter-spacing="1.4" fill="{TEXT3}">ONE AGENT TURN</text>')
    parts.append(f'<text x="{W - 40}" y="37" text-anchor="end" font-family={MONO!r} font-size="10.5" letter-spacing="1.4" fill="{TEXT3}">COMPANY GRAPH · LIVE</text>')
    parts.append(f'<circle cx="{W - 205}" cy="33" r="3" fill="{BLUE}"><animate attributeName="opacity" values="1;0.3;1" dur="2s" repeatCount="indefinite"/></circle>')

    g = f'<g font-family={MONO!r}>'
    parts.append(g)

    LX = 38
    # ---- 1. the ask, typed
    parts.append(f'<text x="{LX}" y="84" font-size="11.5" fill="{BLUE}" font-weight="600">&#10095;</text>')
    svg, t = typed("fix the token-refresh bug in auth", LX + 18, 84, 0.4, 16, DUR, INK, size=12.5)
    parts.append(svg)

    # ---- 2. injection block (the product moment)
    t += 0.5
    bx, by = LX, 102
    inj_head = (f'<rect x="{bx - 8}" y="{by}" width="452" height="118" rx="8" fill="{BLUE100}" fill-opacity="0.55" stroke="{BLUE300}" stroke-opacity="0.6"/>'
                f'<text x="{bx + 6}" y="{by + 19}" font-size="10" letter-spacing="1.2" fill="{MARK}">BASEMODE · PLACED BEFORE THE MODEL ANSWERED</text>')
    parts.append(fade(inj_head, t, DUR))

    rows = [
        ("ast", f"auth.rs — 8 entities · imported by api.rs, middleware.rs", 0.55),
        ("decision", "JWT over sessions — refresh rotates, never reissues", 1.15),
        ("rule", "auth changes require a decision log entry", 1.75),
        ("gotcha", "token clock-skew: tests pass local, fail in CI", 2.35),
    ]
    for i, (tag, line, dt) in enumerate(rows):
        y = by + 42 + i * 21
        row = (f'<text x="{bx + 6}" y="{y}" font-size="11" fill="{BLUE}" font-weight="600">{tag}</text>'
               f'<text x="{bx + 66}" y="{y}" font-size="11" fill="{TEXT2}">{line}</text>')
        parts.append(fade(row, t + dt, DUR))

    # ---- 3. the answer, typed after the injection
    t2 = t + 3.3
    svg, t3 = typed("Rotating refresh per the March decision —", LX, 248, t2, 22, DUR, INK, size=12.5)
    parts.append(svg)
    svg, t4 = typed("patching validate_token with the skew check.", LX, 268, t3 + 0.1, 22, DUR, INK, size=12.5)
    parts.append(svg)

    stats = (f'<text x="{LX}" y="298" font-size="10.5" fill="{TEXT3}">0 files read to orient · context found it first</text>')
    parts.append(fade(stats, t4 + 0.4, DUR))

    # ---- prompt cursor
    parts.append(f'<rect x="{LX}" y="318" width="7" height="13" fill="{INK}"><animate attributeName="opacity" values="1;1;0;0" keyTimes="0;0.5;0.5;1" dur="1.2s" repeatCount="indefinite"/></rect>')

    parts.append("</g>")

    # ---- right pane: the graph, engraved
    nodes = {
        "auth.rs":       (705, 150, 22, True),
        "middleware.rs": (610, 95, 13, False),
        "api.rs":        (800, 95, 13, False),
        "config.rs":     (595, 205, 11, False),
        "database.rs":   (812, 205, 11, False),
        "decision":      (652, 275, 13, False),
        "rule":          (762, 275, 13, False),
    }
    edges = [
        ("auth.rs", "middleware.rs", 0.55), ("auth.rs", "api.rs", 0.55),
        ("auth.rs", "config.rs", 0.55), ("auth.rs", "database.rs", 0.55),
        ("auth.rs", "decision", 1.15), ("auth.rs", "rule", 1.75),
        ("middleware.rs", "api.rs", None), ("config.rs", "decision", None),
        ("database.rs", "rule", None),
    ]
    for a, b, dt in edges:
        x1, y1 = nodes[a][:2]
        x2, y2 = nodes[b][:2]
        parts.append(f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{LINE}" stroke-width="1.2"/>')
        if dt is not None:
            lit = (f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{BLUE}" stroke-width="1.6" stroke-opacity="0.7"/>')
            parts.append(fade(lit, t + dt, DUR))

    for name, (x, y, r, center) in nodes.items():
        base_fill = RAISED
        stroke = TEXT3 if not center else BLUE
        parts.append(f'<circle cx="{x}" cy="{y}" r="{r}" fill="{base_fill}" stroke="{stroke}" stroke-width="1.4"/>')
        dt = dict((e[1], e[2]) for e in edges if e[0] == "auth.rs").get(name)
        if center:
            glow = (f'<circle cx="{x}" cy="{y}" r="{r + 8}" fill="none" stroke="{BLUE}" stroke-opacity="0.35" stroke-width="6"/>'
                    f'<circle cx="{x}" cy="{y}" r="{r}" fill="{BLUE100}" stroke="{BLUE}" stroke-width="1.6"/>')
            parts.append(fade(glow, t + 0.45, DUR))
        elif dt is not None:
            lit = f'<circle cx="{x}" cy="{y}" r="{r}" fill="{BLUE100}" stroke="{BLUE}" stroke-width="1.5"/>'
            parts.append(fade(lit, t + dt + 0.15, DUR))
        label_y = y + r + (24 if center else 14)
        parts.append(f'<text x="{x}" y="{label_y}" text-anchor="middle" font-family={MONO!r} font-size="9" fill="{TEXT3}">{name}</text>')

    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" role="img" '
            f'aria-label="a basemode briefing landing in an agent turn while the graph lights up">'
            + "".join(parts) + "</svg>")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--font", required=True, help="path to Literata variable TTF")
    ap.add_argument("--out", default="docs", help="output directory")
    ap.add_argument("--wordmark", default="basemode", help="splash wordmark text (full stop appended)")
    args = ap.parse_args()

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "splash.svg").write_text(build_splash(args.font, args.wordmark))
    (out / "demo.svg").write_text(build_demo())
    print(f"wrote {out / 'splash.svg'} and {out / 'demo.svg'}")


if __name__ == "__main__":
    main()
