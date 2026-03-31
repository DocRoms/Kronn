---
name: terraform
description: "Use when writing or reviewing Terraform/OpenTofu HCL files, managing cloud infrastructure, or planning IaC changes. Covers state, modules, and provisioning gotchas."
license: AGPL-3.0
category: domain
icon: 🏗️
builtin: true
---

## Procedures

1. **Always plan first** — `terraform plan -out=tfplan`, review, then `terraform apply tfplan`. Never blind-apply.
2. **Modules** — small, versioned, with input validation (`variable` blocks with `validation {}`). Outputs for inter-module wiring.
3. **Environments** — directory-per-env or workspaces. Values in `.tfvars` files, never hardcoded.
4. **Secrets** — never in code or state. Use `sensitive = true`, Vault, or SSM Parameter Store. Audit with `terraform show`.
5. **Testing** — `terraform validate` + `tflint` in CI. `terraform test` or Terratest for integration.

## Gotchas

- `count` creates indexed resources (`[0]`, `[1]`) — removing middle items shifts ALL indexes and forces recreation. Use `for_each` with named keys instead.
- State drift: manual console changes make `plan` lie. Run `terraform refresh` or import before trusting plan output.
- `terraform destroy` has no undo — and `-auto-approve` skips confirmation. Never script `destroy -auto-approve` in CI.
- Provider version upgrades can change resource schemas — pin versions (`required_providers`) and upgrade deliberately.
- `depends_on` on modules forces full re-evaluation — prefer implicit dependencies via resource references.
- Renaming a resource without `moved {}` block = destroy + recreate. Always add `moved { from = ... to = ... }`.

## Validation

- `terraform plan` shows zero unexpected changes on an unchanged codebase.
- All resources tagged (team, env, managed-by).
- No `count` where `for_each` is viable.

## Do/Don't

✓ `for_each = toset(var.bucket_names)` — stable keys, safe to reorder
✗ `count = length(var.bucket_names)` — index shift destroys resources
✓ `moved { from = aws_s3_bucket.old to = aws_s3_bucket.new }`
✗ Rename resource without `moved` — silent destroy + recreate
