- **ID**: TD-20260510-codex-upstream-issue-draft
- **Area**: External / upstream
- **Severity**: Low (dependency on TD-20260510-codex-mcp-sandbox-block)
- **Status**: 🟢 Draft prepared — READY TO FILE at https://github.com/openai/codex-cli (0.8.11 audit follow-up). This is an UPSTREAM post, not internal code — it needs the maintainer's GitHub account to submit; once filed, close this TD. No further code action on the Kronn side.

## Purpose

Companion to `TD-20260510-codex-mcp-sandbox-block`. We need an upstream
issue describing the failure mode so Codex 0.121 either fixes the gate
or documents how to work around it. This file holds the draft body
ready to paste into a new GitHub issue.

## Draft body

> ### Codex 0.121 cancels MCP tool calls in `exec` mode despite `approval: never`
>
> #### Repro
>
> 1. Add an MCP server to `~/.codex/config.toml`:
>    ```toml
>    [mcp_servers.example-bridge]
>    command = "python3"
>    args = ["/abs/path/to/bridge.py"]
>    startup_timeout_sec = 30
>    ```
> 2. `codex mcp list` shows the entry as `enabled` ✅
> 3. Run `codex exec --skip-git-repo-check "Call example-bridge.some_tool"`.
> 4. Observed: Codex banner reads `approval: never`. The model attempts
>    the call, the runtime logs `mcp: example-bridge/some_tool started`,
>    immediately followed by `(failed)` and `user cancelled MCP tool call`.
>
> #### Expected
>
> With `--ask-for-approval=never` (the documented default for non-interactive
> exec runs, per the help text), MCP tool calls should auto-approve and
> let the bridge subprocess run.
>
> #### Workarounds tried
>
> - `-s danger-full-access`: same outcome.
> - `--full-auto`: same outcome.
> - `-c approval_policy.mcp_servers.example-bridge.auto_approve=true`:
>   key not recognised by the TOML schema parser (so I'm unsure if it's
>   meant to exist).
>
> #### Why this matters
>
> Self-hosted projects (Kronn, in our case) wire an in-process MCP
> bridge so the agent can introspect its own session DB. The bridge
> works for Claude Code, Kiro, Gemini CLI, Copilot CLI — Codex is the
> only one that reads its config but refuses to invoke. Forces us to
> either ship a degraded UX for Codex users, or recommend they avoid
> Codex.
>
> Happy to PR if you can point me at the right policy gate.
>
> Versions: codex 0.121.0, model gpt-5.3-codex, Linux x86_64.

## Where (pointers)

- `backend/src/agents/runner.rs:846-883` — current Kronn-side spawn
  args. Already passes `approval: never` via the banner; not enough.
- `backend/src/core/mcp_scanner.rs:1085-1175` — the `CodexSync` impl
  that writes the entry to `~/.codex/config.toml`. Verified
  end-to-end via `codex mcp list`.
- `~/.local/share/rtk/tee/<run>_codex.log` — sample failed run if
  the user wants to attach trace to the upstream issue.

## One-click pre-filled URL

The body above is pre-encoded into a GitHub issue URL — clicking
opens a new-issue form already filled in. No copy-paste needed.

[Open pre-filled issue at openai/codex-cli](https://github.com/openai/codex-cli/issues/new?title=Codex%200.121%20cancels%20MCP%20tool%20calls%20in%20exec%20mode%20despite%20approval%3A%20never&body=%23%23%23%20Codex%200.121%20cancels%20MCP%20tool%20calls%20in%20%60exec%60%20mode%20despite%20%60approval%3A%20never%60%0A%0A%23%23%23%23%20Repro%0A%0A1.%20Add%20an%20MCP%20server%20to%20%60~/.codex/config.toml%60%3A%0A%20%20%20%60%60%60toml%0A%20%20%20%5Bmcp_servers.example-bridge%5D%0A%20%20%20command%20%3D%20%22python3%22%0A%20%20%20args%20%3D%20%5B%22/abs/path/to/bridge.py%22%5D%0A%20%20%20startup_timeout_sec%20%3D%2030%0A%20%20%20%60%60%60%0A2.%20%60codex%20mcp%20list%60%20shows%20the%20entry%20as%20%60enabled%60%20%E2%9C%85%0A3.%20Run%20%60codex%20exec%20--skip-git-repo-check%20%22Call%20example-bridge.some_tool%22%60.%0A4.%20Observed%3A%20Codex%20banner%20reads%20%60approval%3A%20never%60.%20The%20model%20attempts%0A%20%20%20the%20call%2C%20the%20runtime%20logs%20%60mcp%3A%20example-bridge/some_tool%20started%60%2C%0A%20%20%20immediately%20followed%20by%20%60%28failed%29%60%20and%20%60user%20cancelled%20MCP%20tool%20call%60.%0A%0A%23%23%23%23%20Expected%0A%0AWith%20%60--ask-for-approval%3Dnever%60%20%28the%20documented%20default%20for%20non-interactive%0Aexec%20runs%2C%20per%20the%20help%20text%29%2C%20MCP%20tool%20calls%20should%20auto-approve%20and%0Alet%20the%20bridge%20subprocess%20run.%0A%0A%23%23%23%23%20Workarounds%20tried%0A%0A-%20%60-s%20danger-full-access%60%3A%20same%20outcome.%0A-%20%60--full-auto%60%3A%20same%20outcome.%0A-%20%60-c%20approval_policy.mcp_servers.example-bridge.auto_approve%3Dtrue%60%3A%0A%20%20key%20not%20recognised%20by%20the%20TOML%20schema%20parser%20%28so%20I%27m%20unsure%20if%20it%27s%0A%20%20meant%20to%20exist%29.%0A%0A%23%23%23%23%20Why%20this%20matters%0A%0ASelf-hosted%20projects%20%28Kronn%2C%20in%20our%20case%29%20wire%20an%20in-process%20MCP%0Abridge%20so%20the%20agent%20can%20introspect%20its%20own%20session%20DB.%20The%20bridge%0Aworks%20for%20Claude%20Code%2C%20Kiro%2C%20Gemini%20CLI%2C%20Copilot%20CLI%20%E2%80%94%20Codex%20is%20the%0Aonly%20one%20that%20reads%20its%20config%20but%20refuses%20to%20invoke.%20Forces%20us%20to%0Aeither%20ship%20a%20degraded%20UX%20for%20Codex%20users%2C%20or%20recommend%20they%20avoid%0ACodex.%0A%0AHappy%20to%20PR%20if%20you%20can%20point%20me%20at%20the%20right%20policy%20gate.%0A%0AVersions%3A%20codex%200.121.0%2C%20model%20gpt-5.3-codex%2C%20Linux%20x86_64.)

## Next step

Click the URL above to open a pre-filled issue at
`openai/codex-cli`. Link the resulting issue number back to
TD-20260510-codex-mcp-sandbox-block. Once filed, drop this TD from
the index — the "next-step" is consumed by a single click.
