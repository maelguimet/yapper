#!/usr/bin/python3
# -*- coding: utf-8 -*-
"""pytest console entry — user-site from install prefix, not HOME.

When integration tests set HOME to an empty temp dir (path isolation for
XDG / Eve assets), the default user-site (~$HOME/.local/...) disappears and
this entrypoint would fail to import _pytest. Resolve site-packages relative
to this script (~/.local/bin → ~/.local/lib/pythonX.Y/site-packages) so the
test runner stays available while app code still sees the empty HOME.
"""
import re
import sys
from pathlib import Path


def _ensure_install_user_site() -> None:
    bindir = Path(__file__).resolve().parent
    local = bindir.parent
    ver = f"python{sys.version_info.major}.{sys.version_info.minor}"
    user_site = local / "lib" / ver / "site-packages"
    if user_site.is_dir():
        path = str(user_site)
        if path not in sys.path:
            sys.path.insert(0, path)


_ensure_install_user_site()

from _pytest.config import _console_main

if __name__ == "__main__":
    sys.argv[0] = re.sub(r"(-script\.pyw|\.exe)?$", "", sys.argv[0])
    sys.exit(_console_main())
