Read docs/AGENTS.md for context. Find operational commands from Makefile, package.json scripts, docker-compose commands, and any run/build/debug procedures.

Fill docs/operations/debug-operations.md — replace ALL {{PLACEHOLDERS}}:
- Common commands: replace the {{COMMANDS}} block with a table of action → command for start, stop, logs, test, build, deploy
- Docker services: replace the {{DOCKER_SERVICES}} block with a table of service, port, role, and health check for each container
- Troubleshooting: replace the {{TROUBLESHOOTING}} block with 3-5 common issues (symptom, cause, fix)