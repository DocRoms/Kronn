# Windows-native Kronn with agents installed in WSL

Kronn running as a Windows program (desktop app or native backend) can start an
agent CLI that exists only inside WSL. `find_binary` finds it through
`wsl.exe` (`via_wsl`) and the launch becomes
`wsl.exe --cd <linux dir> -e <agent> <args…>`.
[src: file: backend/src/agents/mod.rs:1159-1222]
[src: file: backend/src/agents/wsl.rs:1]

Kronn running *inside* WSL (Linux build) is not affected: nothing crosses the
boundary.

## What Kronn does at the boundary

- **Environment.** `wsl.exe` drops every Windows variable that `WSLENV` does
  not list. Each launch therefore adds to `WSLENV` the name of every variable
  it sets (discussion id, backend URL, worker/room/workflow contexts, API key,
  GitHub token, the MCP secret references of KT-964/KT-1003, the scoped
  `KRONN_BRIDGE_TOKEN`, `TMPDIR`…). The launch's environment is built, not
  inherited (KT-1006), so that list is complete; the Windows-side allow-list
  (`SystemRoot`, proxies, locale…) stays on the Windows side. Names only, never
  values. The user's existing `WSLENV` is kept and their flags win for a name
  they already list.
  `TMPDIR`/`TEMP`/`TMP` carry `/up` (translated, Linux side only). Other
  names carry no flag, so they also reach a Windows program the agent starts
  through interop, such as the bundled `kronn-internal.exe` bridge. `HOME`,
  `USERPROFILE`, `COPILOT_HOME`, `PATH` and `SHELL` are never forwarded: WSL
  keeps its own.
- **Paths in arguments.** Windows paths become WSL paths (`C:\x` →
  `/mnt/c/x`, `\\wsl.localhost\Ubuntu\home\x` → `/home/x`): whole path
  arguments (`--add-dir`, a `--mcp-config` file), strings inside the inline
  JSON of `--mcp-config` and `--settings` (the bridge command or script, the
  sandbox roots) and inside Codex `-c` TOML overrides (bridge command, trusted
  project keys). A bundled `kronn-internal.exe` then runs from WSL through
  interop; a `python3 <script>` bridge runs on the Linux side.
- **Backend URL.** A WSL agent gets `KRONN_WSL_BACKEND_URL` when it is set,
  otherwise the usual `KRONN_BACKEND_URL` (default `http://127.0.0.1:3140`).

## Networking: NAT versus mirrored

In WSL2's default NAT mode, `127.0.0.1` inside WSL is the Linux VM, not
Windows. A bridge running on the Linux side (the `python3` script, used by
source checkouts) cannot reach a backend bound to Windows loopback. The bundled
`kronn-internal.exe` runs on the Windows side through interop and reaches it
anyway. Kronn logs one warning per process when an agent runs in WSL, no
override is set and `%USERPROFILE%\.wslconfig` does not select mirrored mode.

Fixes, in order of preference:

1. Mirrored networking: in `%USERPROFILE%\.wslconfig`
   ```ini
   [wsl2]
   networkingMode=mirrored
   ```
   then `wsl --shutdown`. Loopback is shared and the default URL works.
2. Keep NAT, bind the backend to an address WSL can reach (the Windows host's
   `vEthernet (WSL)` address, see `ip route show default` inside WSL) with
   `KRONN_HOST`, and set `KRONN_WSL_BACKEND_URL=http://<that address>:3140`.
   Binding beyond loopback exposes the API: keep authentication on. Kronn never
   binds `0.0.0.0` by default.

## Native ACP agents

Gemini, Copilot, Kiro, OpenCode and Vibe use their own ACP runtime, which Kronn
spawns as a Windows program, and their ACP messages carry Windows paths
(`cwd`, MCP servers). They are not routed through WSL. When one is found only
inside WSL, the launch is refused with a message naming the binary: install it
on Windows, or run Kronn inside WSL.
[src: file: backend/src/acp.rs:722]

## Limits

- The opt-out direct Claude route (`--mcp-config <file>`) gets the file path
  translated, but the file's contents are read as written. Its
  `kronn-internal` command is a Windows path unless the bridge is reachable
  under the same string. The default ACP adapter route passes inline JSON and
  is fully translated.
- A project on a non-WSL UNC share (`\\server\share`) has no WSL path; the
  argument is passed unchanged.

## Manual trial (Windows + WSL)

Needs a Windows 11 machine with WSL2, a distribution with `python3`, and Claude
Code (`claude`) installed **only** inside WSL (`which claude` in WSL succeeds,
`where claude` in PowerShell fails). Optionally Codex the same way.

1. Start Kronn on Windows (desktop app or `kronn.exe`). Open the Agents page:
   Claude is detected with a WSL label.
2. NAT check first: with no `.wslconfig` mirrored setting and no
   `KRONN_WSL_BACKEND_URL`, start a discussion with Claude on a project under
   `C:\…`. Expect the backend log line "An agent runs in WSL and WSL networking
   is not mirrored". With the installed (bundled) bridge, ask Claude to call a
   `kronn-internal` tool such as `kronn_intro`: it should answer.
3. Ask Claude to run `env | grep -E 'KRONN_|TMPDIR'` and `pwd`. Expect
   `KRONN_DISCUSSION_ID`, `KRONN_BACKEND_URL`, `KRONN_BRIDGE_TOKEN`,
   `TMPDIR=/mnt/c/…/.kronn/tmp` and a `/mnt/c/…` working directory, and never
   `KRONN_AUTH_TOKEN`.
   `echo $WSLENV` lists names only.
4. Ask Claude to list its MCP servers (`/mcp` is not available in print mode:
   ask it to call a tool of each project MCP). The project's MCP servers from
   `.mcp.json` and `kronn-internal` must be present. Repeat on a project whose
   agent files live outside the repository (KT-971).
5. Switch to mirrored mode (`.wslconfig`, `wsl --shutdown`), restart Kronn,
   repeat step 2: no warning, tools still work.
6. Back in NAT mode, run from a source checkout (the `python3` bridge): tools
   fail to reach the backend. Bind the backend to the WSL host address with
   auth on, set `KRONN_WSL_BACKEND_URL`, restart: tools work.
7. Start a Claude task worker (Windows always routes workers through WSL): the
   worker's sandbox settings show `/mnt/c/…` roots and its delivery tool works.
8. With Gemini installed only inside WSL, start a Gemini discussion: expect the
   refusal "`gemini` is installed only inside WSL…".
9. Set `WSLENV=GOPATH/l` in the Windows user environment before starting
   Kronn, repeat step 3: `GOPATH/l` is still first in `$WSLENV`.
