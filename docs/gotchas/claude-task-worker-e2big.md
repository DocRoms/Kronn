# Claude task-worker sandbox and large worktree catalogues

Punctual Claude Code task workers require Claude Code `2.1.247` or newer. The
runner checks the installed version before authentication and returns a
`task_exec_reassign` recovery instruction when the build is older or its
version cannot be verified. Ordinary Claude Code discussions are not subject
to this task-worker-only gate. [src: file: backend/src/core/versions.rs:36-43]
[src: file: backend/src/agents/runner.rs:7062-7119]

The worker remains fail-closed: sandbox availability is mandatory,
unsandboxed commands are disabled, and the only explicit write root is the
canonical managed task worktree. [src: file: backend/src/agents/runner.rs:6997-7019]

Regression coverage constructs an unrelated synthetic worktree catalogue
larger than Kronn's single-argument guard and verifies that it is absent from
the invocation while the sandbox write root remains the task worktree.
[src: file: backend/src/agents/runner_test.rs:5447-5508]
