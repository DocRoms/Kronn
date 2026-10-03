# CI/CD pipeline

> **TEMPLATE FILE.** Fill `{{...}}` with the actual project convention. If it isn't formalized yet, write `Not formalized — ask before assuming.` instead of guessing.

## Pipeline stages

<!-- Fill: build → test → deploy stages, the provider (GitHub Actions, GitLab CI, etc.), and where the config lives -->
{{PIPELINE_STAGES}}

## Quality gates

<!-- Fill: what blocks a merge/deploy (tests, lint, security scan, manual approval) -->
{{QUALITY_GATES}}

## Deployment targets

<!-- Fill: which environment each branch/tag deploys to. Full environment detail lives in ../environments.md — do not duplicate it here. -->
{{DEPLOY_TARGETS}}
