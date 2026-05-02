"""Entry-point script that execs the bundled native binary.

Registered in pyproject.toml as `[project.scripts] unbrowser = ...`, so
`pip install pyunbrowser` puts a real `unbrowser` command on $PATH that
agents and MCP hosts can use directly (e.g. `command: "unbrowser"` in
.mcp.json).

Uses os.execv so this Python process is replaced wholesale by the binary —
no double-fork, no stdin/stdout marshalling overhead.
"""

from __future__ import annotations

import os
import sys

from . import find_binary


def main() -> None:
    binary = find_binary()
    # Replace this Python process with the binary. argv[0] gets set to
    # 'unbrowser' so error messages, ps output, etc. show the right name.
    os.execv(binary, ["unbrowser", *sys.argv[1:]])


if __name__ == "__main__":
    main()
