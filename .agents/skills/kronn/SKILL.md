---
name: kronn
description: Kronn Workflow Router
---

# Kronn Workflow Router

This skill acts as the entry point and router for Kronn portable workflows.

## Available Workflows

Workflows are stored in `.agents/workflows/`. To list them, run:

```bash
ls -1 .agents/workflows/*.json 2>/dev/null || echo "No workflows found."
```

## Invocation

To run a workflow reproducibly, use the `kronn run` command and pass variables using `--var`:

```bash
kronn run .agents/workflows/my-workflow.json --var ENV_VAR=value
```

### Container Fallback

If you cannot run the workflow natively (e.g., missing dependencies listed in `requires`), you can use the documented container fallback:

```bash
docker run --rm -it \
  -v "$(pwd):/workspace" \
  -w /workspace \
  -e ENV_VAR=value \
  kronn/runner:latest \
  kronn run .agents/workflows/my-workflow.json
```

## Bootstrap

Before running a workflow for the first time, you can bootstrap the environment (which generates `.env.example` and checks dependencies):

```bash
sh .agents/scripts/bootstrap.sh .agents/workflows/my-workflow.json
```
