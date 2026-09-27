"""The sidecar must stop once the backend's end of its stdin pipe closes."""

from __future__ import annotations

import os
import threading
import unittest

from kronn_docs import parent_watch


class ExitWhenClosedTest(unittest.TestCase):
    def test_exits_only_after_the_writer_closes(self) -> None:
        reader_fd, writer_fd = os.pipe()
        exited = threading.Event()
        codes: list[int] = []

        def fake_exit(code: int) -> None:
            codes.append(code)
            exited.set()

        with os.fdopen(reader_fd, "rb", buffering=0) as reader:
            watcher = threading.Thread(
                target=parent_watch.exit_when_closed, args=(reader, fake_exit), daemon=True
            )
            watcher.start()
            os.write(writer_fd, b"noise that must not stop the sidecar\n")
            self.assertFalse(exited.wait(0.3), "exited while the writer was still open")
            os.close(writer_fd)
            self.assertTrue(exited.wait(5), "did not exit after the writer closed")
            watcher.join(5)
        self.assertEqual(codes, [0])

    def test_watch_is_opt_in(self) -> None:
        self.assertFalse(parent_watch.start_if_requested({}))
        self.assertFalse(parent_watch.start_if_requested({parent_watch.ENV_FLAG: "0"}))


CHILD = (
    "import time\n"
    "from kronn_docs import parent_watch\n"
    "parent_watch.start_if_requested()\n"
    "print('ready', flush=True)\n"
    "time.sleep(60)\n"
)

# The parent holds the child's stdin pipe, like the Rust backend, then waits.
PARENT = (
    "import os, subprocess, sys, time\n"
    "env = dict(os.environ)\n"
    "if sys.argv[1] == '1':\n"
    f"    env[{parent_watch.ENV_FLAG!r}] = '1'\n"
    "child = subprocess.Popen([sys.executable, '-c', sys.argv[2]], env=env,\n"
    "                         stdin=subprocess.PIPE, stdout=subprocess.PIPE)\n"
    "child.stdout.readline()\n"
    "print(child.pid, flush=True)\n"
    "time.sleep(60)\n"
)


@unittest.skipIf(os.name != "posix", "SIGKILL and orphan adoption are POSIX behaviour")
class ParentKilledOutrightTest(unittest.TestCase):
    def _child_after_parent_sigkill(self, watch: bool) -> tuple[int, bool]:
        import signal
        import subprocess
        import sys
        import time

        env = dict(os.environ, PYTHONPATH=os.path.dirname(os.path.abspath(__file__)))
        parent = subprocess.Popen(
            [sys.executable, "-c", PARENT, "1" if watch else "0", CHILD],
            env=env,
            stdout=subprocess.PIPE,
        )
        child_pid = int(parent.stdout.readline())
        parent.stdout.close()
        os.kill(parent.pid, signal.SIGKILL)
        parent.wait(5)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            try:
                os.kill(child_pid, 0)
            except ProcessLookupError:
                return child_pid, False
            time.sleep(0.1)
        return child_pid, True

    def test_sidecar_stops_when_its_parent_is_killed(self) -> None:
        child_pid, alive = self._child_after_parent_sigkill(watch=True)
        if alive:
            os.kill(child_pid, 9)
        self.assertFalse(alive, "sidecar survived its parent's SIGKILL")

    def test_without_the_watch_the_sidecar_is_orphaned(self) -> None:
        # Guards the test itself: the scenario must reproduce the leak.
        child_pid, alive = self._child_after_parent_sigkill(watch=False)
        if alive:
            os.kill(child_pid, 9)
        self.assertTrue(alive, "the scenario no longer reproduces an orphan")


if __name__ == "__main__":
    unittest.main()
