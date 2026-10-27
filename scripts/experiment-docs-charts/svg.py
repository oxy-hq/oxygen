"""Tiny dependency-free SVG chart helpers, themed to read on light and dark pages."""
from html import escape

TEAL = "#0d9488"
ORANGE = "#ea8a2e"
SLATE = "#8792a2"
RED = "#dc4a4a"
BLUE = "#3b82f6"
TEXT = "#7b8494"
GRID = "#8792a2"
FONT = "font-family='Inter, system-ui, -apple-system, Segoe UI, sans-serif'"


class Chart:
    def __init__(self, w, h, x0, x1, y0, y1, left=56, right=20, top=36, bottom=44, title=None):
        self.w, self.h = w, h
        self.x0, self.x1, self.y0, self.y1 = x0, x1, y0, y1
        self.l, self.r, self.t, self.b = left, right, top, bottom
        self.parts = []
        self.title = title

    def X(self, x):
        return self.l + (x - self.x0) / (self.x1 - self.x0) * (self.w - self.l - self.r)

    def Y(self, y):
        return self.h - self.b - (y - self.y0) / (self.y1 - self.y0) * (self.h - self.t - self.b)

    def add(self, s):
        self.parts.append(s)

    def text(self, x, y, s, size=12, anchor="start", color=TEXT, weight=400, px=False):
        X = x if px else self.X(x)
        Y = y if px else self.Y(y)
        self.add(f"<text x='{X:.1f}' y='{Y:.1f}' font-size='{size}' text-anchor='{anchor}' fill='{color}' font-weight='{weight}' {FONT}>{escape(s)}</text>")

    def grid(self, yticks, fmt=lambda v: f"{v:g}", xticks=None, xfmt=lambda v: f"{v:g}", xlabel=None, ylabel=None):
        for v in yticks:
            y = self.Y(v)
            self.add(f"<line x1='{self.l}' x2='{self.w - self.r}' y1='{y:.1f}' y2='{y:.1f}' stroke='{GRID}' stroke-opacity='0.25' stroke-width='1'/>")
            self.text(self.l - 8, y + 4, fmt(v), size=11, anchor="end", px=True)
        for v in xticks or []:
            x = self.X(v)
            self.text(x, self.h - self.b + 16, xfmt(v), size=11, anchor="middle", px=True)
        if xlabel:
            self.text((self.l + self.w - self.r) / 2, self.h - 8, xlabel, size=11, anchor="middle", px=True)
        if ylabel:
            self.add(f"<text transform='translate(14,{(self.t + self.h - self.b) / 2:.1f}) rotate(-90)' font-size='11' text-anchor='middle' fill='{TEXT}' {FONT}>{escape(ylabel)}</text>")

    def span(self, xa, xb, color=TEAL, opacity=0.10, label=None, label_y=None):
        a, b = sorted((self.X(xa), self.X(xb)))
        self.add(f"<rect x='{a:.1f}' y='{self.t}' width='{b - a:.1f}' height='{self.h - self.t - self.b}' fill='{color}' fill-opacity='{opacity}'/>")
        if label:
            self.text((a + b) / 2, label_y or self.t + 14, label, size=11, anchor="middle", px=True)

    def vline(self, x, color=TEXT, dash="4 3", label=None, width=1.2):
        X = self.X(x)
        self.add(f"<line x1='{X:.1f}' x2='{X:.1f}' y1='{self.t}' y2='{self.h - self.b}' stroke='{color}' stroke-width='{width}' stroke-dasharray='{dash}'/>")
        if label:
            self.text(X + 4, self.t + 28, label, size=11, color=color, px=True)

    def hline(self, y, color=TEXT, dash="4 3", width=1.2):
        Y = self.Y(y)
        self.add(f"<line x1='{self.l}' x2='{self.w - self.r}' y1='{Y:.1f}' y2='{Y:.1f}' stroke='{color}' stroke-width='{width}' stroke-dasharray='{dash}'/>")

    def line(self, pts, color, width=2, opacity=1.0, dash=None):
        d = " ".join(f"{'M' if i == 0 else 'L'}{self.X(x):.1f},{self.Y(y):.1f}" for i, (x, y) in enumerate(pts))
        da = f" stroke-dasharray='{dash}'" if dash else ""
        self.add(f"<path d='{d}' fill='none' stroke='{color}' stroke-width='{width}' stroke-opacity='{opacity}' stroke-linejoin='round'{da}/>")

    def dot(self, x, y, color, r=3.5, opacity=1.0):
        self.add(f"<circle cx='{self.X(x):.1f}' cy='{self.Y(y):.1f}' r='{r}' fill='{color}' fill-opacity='{opacity}'/>")

    def rect(self, xa, xb, ya, yb, color, opacity=1.0):
        a, b = sorted((self.X(xa), self.X(xb)))
        c, d = sorted((self.Y(ya), self.Y(yb)))
        self.add(f"<rect x='{a:.1f}' y='{c:.1f}' width='{b - a:.1f}' height='{d - c:.1f}' fill='{color}' fill-opacity='{opacity}' rx='2'/>")

    def legend(self, items, x=None, y=None):
        x = self.l + 4 if x is None else x
        y = 18 if y is None else y
        for label, color, kind in items:
            if kind == "line":
                self.add(f"<line x1='{x}' x2='{x + 18}' y1='{y - 4}' y2='{y - 4}' stroke='{color}' stroke-width='3'/>")
            elif kind == "dash":
                self.add(f"<line x1='{x}' x2='{x + 18}' y1='{y - 4}' y2='{y - 4}' stroke='{color}' stroke-width='2' stroke-dasharray='4 3'/>")
            else:
                self.add(f"<rect x='{x}' y='{y - 10}' width='14' height='12' fill='{color}' fill-opacity='{0.25 if kind == 'span' else 1}' rx='2'/>")
            self.text(x + 24, y, label, size=12, px=True)
            x += 34 + 7 * len(label)

    def svg(self):
        title = f"<title>{escape(self.title)}</title>" if self.title else ""
        return (f"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 {self.w} {self.h}' width='100%' role='img'>{title}"
                + "".join(self.parts) + "</svg>")
