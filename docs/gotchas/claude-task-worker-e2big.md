# Claude task-worker sandbox and large worktree catalogues

Claude Code `2.1.247` reproduced the sandbox-internal `E2BIG` failure during a
real task-worker run, so a successful `claude --version` command is not evidence
that sandboxed Bash can spawn. The upstream defect remains open.
[src: user: 2026-08-28: KT-492 dogfood and review feedback]
[src: url: https://github.com/anthropics/claude-code/issues/73437]
[src: url: https://github.com/anthropics/claude-code/issues/73468]

On macOS, Kronn conservatively measures the `projects` catalogue in the host
Claude configuration before provisioning a punctual Claude worker. A catalogue
over either bound is refused with the stable
`claude_sandbox_catalogue_unsafe` reason, counts and byte totals only, plus a
`task_exec_reassign` recovery instruction. Missing or malformed catalogue data
also fails closed. [src: file: backend/src/agents/runner.rs:7062]
[src: file: backend/src/api/orchestration.rs:5454]

The worker remains fail-closed: sandbox availability is mandatory,
unsandboxed commands are disabled, and the only explicit write root is the
canonical managed task worktree. [src: file: backend/src/agents/runner.rs:6997-7019]

Regression coverage exercises accepted and refused catalogue sizes, verifies
that diagnostics contain no paths, and separately preserves the single
task-worktree write boundary. [src: file: backend/src/agents/runner_test.rs:5399]
