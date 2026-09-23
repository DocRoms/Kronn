#!/usr/bin/env python3
"""Run MCP from a relocated bundle, without source checkout or system Python."""

import http.server
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
from pathlib import Path


def smoke_bundle(executable):
    executable = Path(executable).resolve()
    requests = []

    class Backend(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            requests.append(self.path)
            data = json.dumps({
                "success": True,
                "data": {"id": "packaged-bridge", "instance": "ephemeral-desktop"},
            }).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *_args):
            pass

    with tempfile.TemporaryDirectory(prefix="Kronn MCP é ") as directory:
        root = Path(directory).resolve()
        installed = root / "Installed app with spaces"
        shutil.copytree(executable.parent, installed)
        launch = installed / executable.name
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Backend)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        env = {key: value for key, value in os.environ.items()
               if key.upper() in {"SYSTEMROOT", "WINDIR", "SYSTEMDRIVE",
                                  "COMSPEC", "PATHEXT", "LANG", "LC_ALL"}}
        env.update({
            "PATH": "", "HOME": str(root), "USERPROFILE": str(root),
            "APPDATA": str(root), "TEMP": str(root), "TMP": str(root),
            "KRONN_BACKEND_URL": f"http://127.0.0.1:{server.server_port}",
        })
        messages = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize",
             "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                        "clientInfo": {"name": "bundle-smoke", "version": "1"}}},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
             "params": {"name": "bridge_info", "arguments": {}}},
            {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
             "params": {"name": "resolve_id", "arguments": {"id": "packaged-bridge"}}},
        ]
        try:
            process = subprocess.run(
                [str(launch)], cwd=root, env=env,
                input="".join(json.dumps(item) + "\n" for item in messages),
                capture_output=True, text=True, timeout=30, check=True,
            )
            replies = {item["id"]: item for line in process.stdout.splitlines()
                       if "id" in (item := json.loads(line))}
            assert "serverInfo" in replies[1]["result"], replies
            for request_id in (2, 3):
                assert not replies[request_id]["result"].get("isError"), replies
            info = json.loads(replies[2]["result"]["content"][0]["text"])
            assert info["stale"] is False, info
            assert Path(info["script_path"]).is_relative_to(installed), info
            resolved = json.loads(replies[3]["result"]["content"][0]["text"])
            assert resolved["instance"] == "ephemeral-desktop", resolved
            assert requests == ["/api/resolve/packaged-bridge"], requests
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)
    print("MCP smoke passed: relocated bundle, empty PATH, fresh bridge, correct backend")


if __name__ == "__main__":
    smoke_bundle(sys.argv[1])
