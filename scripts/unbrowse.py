"""scripts/unbrowse.py — backward-compat shim.

The real implementation lives in ``python/unbrowse/__init__.py`` (the
PyPI-publishable package). This file exists so the SKILL.md pattern and
existing dev scripts keep working without `pip install`:

    sys.path.insert(0, "/path/to/repo/scripts")
    from unbrowse import Client

It's a thin re-export — no client logic lives here.

Implementation note: we load the real package from an explicit file path
via importlib rather than relying on sys.path ordering, so this works
even when an editable install already registered ``unbrowse`` somewhere
else on sys.path (the path-search-order approach silently recursed in
that case).
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

_PKG_INIT = (
    Path(__file__).resolve().parent.parent / "python" / "unbrowse" / "__init__.py"
)

_spec = importlib.util.spec_from_file_location("unbrowse", _PKG_INIT)
if _spec is None or _spec.loader is None:
    raise ImportError(f"unbrowse shim: could not load package from {_PKG_INIT}")
_pkg = importlib.util.module_from_spec(_spec)
sys.modules["unbrowse"] = _pkg
_spec.loader.exec_module(_pkg)

Client = _pkg.Client
UnbrowseError = _pkg.UnbrowseError
find_binary = _pkg.find_binary
navigate = _pkg.navigate
__version__ = _pkg.__version__

__all__ = ["Client", "UnbrowseError", "find_binary", "navigate", "__version__"]
