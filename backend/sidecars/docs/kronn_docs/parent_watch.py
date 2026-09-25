"""Stop the sidecar when the Kronn backend that spawned it disappears.

The backend's kill-on-drop never runs when it is killed outright (SIGKILL,
crash), but the kernel still closes its end of our stdin pipe, so EOF on
stdin is the one signal that always arrives. Dependency-free on purpose.
"""

from __future__ import annotations

import os
import sys
import threading
from typing import BinaryIO, Callable, Mapping

ENV_FLAG = "KRONN_DOCS_EXIT_ON_STDIN_EOF"


def exit_when_closed(stream: BinaryIO, exit: Callable[[int], object] = os._exit) -> None:
    """Block until `stream` reaches EOF, then stop the process at once."""
    while stream.read(4096):
        pass
    exit(0)


def start_if_requested(environ: Mapping[str, str] | None = None) -> bool:
    """Watch stdin in a daemon thread when the spawner asked for it."""
    env = os.environ if environ is None else environ
    if env.get(ENV_FLAG) != "1":
        return False
    threading.Thread(
        target=exit_when_closed,
        args=(sys.stdin.buffer,),
        name="kronn-docs-parent-watch",
        daemon=True,
    ).start()
    return True
