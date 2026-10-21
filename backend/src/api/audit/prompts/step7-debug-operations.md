Read docs/AGENTS.md for context. Find operational commands from Makefile, package.json scripts, docker-compose commands, and any run/build/debug procedures.

Fill docs/operations/debug-operations.md — replace ALL {{PLACEHOLDERS}}:
- Common commands: replace the {{COMMANDS}} block with a table of action → command for start, stop, logs, test, build, deploy
- Docker services: replace the {{DOCKER_SERVICES}} block with a table of service, port, role, and health check for each container
- Troubleshooting: replace the {{TROUBLESHOOTING}} block with 3-5 common issues (symptom, cause, fix)

**ONLY COMMANDS THAT EXIST** — write a command only after reading where this repository defines it: a `make` target must be a target of a Makefile, a `yarn`/`npm`/`pnpm` command a script of a `package.json`, a `composer` command a script of `composer.json`. A package manager that exists only in the dev image is not an application setup step: with no `package.json` there is no `yarn install` to document. Read the recipe of every Makefile target you cite before saying what it does. Kronn checks every command written as code against those files and sends the step back for one it cannot find.
When a command needs a credential (registry token, SSH key, cloud login), name the mechanism the repository's own target and script use (read the target and the script it calls); never name a mechanism from habit. A troubleshooting cause or fix must come from a script, config or code you read.
**CI AND TESTS COME FROM KRONN'S FACTS** — any statement about what CI runs, when it runs (events, labels, branches, schedules) or on which runtime version, and any count of tests, is copied from the `CI facts` and `Test facts` blocks at the end of this prompt, with their file and line; give the same facts as the other documents. A deploy or release step you describe is the one a workflow file defines, with its trigger as the file states it; never add a purpose, a guarantee or a gate the files do not show.
