#!/usr/bin/env python3
"""Minimal Tk text sink for Xvfb paste inject tests.

Writes widget contents to OUT_PATH every 40ms so a test can poll for paste.
Usage: x11_paste_sink.py OUT_PATH
"""

from __future__ import annotations

import sys
from pathlib import Path

import tkinter as tk


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: x11_paste_sink.py OUT_PATH", file=sys.stderr)
        return 2
    out = Path(sys.argv[1])
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("", encoding="utf-8")

    root = tk.Tk()
    root.title("yapper-paste-sink")
    root.geometry("500x250+60+60")
    text = tk.Text(root)
    text.pack(fill="both", expand=True)
    text.focus_force()

    def dump() -> None:
        try:
            body = text.get("1.0", "end-1c")
            out.write_text(body, encoding="utf-8")
        except OSError:
            pass
        root.after(40, dump)

    root.after(40, dump)
    root.mainloop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
