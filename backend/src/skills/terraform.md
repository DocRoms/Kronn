---
name: Terraform
description: Infrastructure as Code with Terraform, state management, and cloud provisioning
category: domain
icon: 🏗️
builtin: true
---

Expert Infrastructure as Code knowledge:

- Terraform: HCL syntax, provider configuration, resource lifecycle. Use `terraform plan` before every `apply`.
- State management: remote backends (S3, GCS, Terraform Cloud). State locking. Never edit state manually — use `terraform state` commands.
- Modules: reusable, versioned modules. Input variables with validation. Outputs for inter-module communication. Keep modules small and focused.
- Environments: workspaces or directory-per-environment. Use `tfvars` files. Never hardcode environment-specific values.
- Security: no secrets in state or code. Use `sensitive = true`, vault integration, or SSM parameters. Least-privilege IAM policies.
- Testing: `terraform validate`, `tflint` for linting, Terratest or `terraform test` for integration tests.
- Alternatives: understand trade-offs with Pulumi (general-purpose languages), CloudFormation (AWS-native), CDK (imperative-to-declarative).
- Best practices: pin provider versions, use `for_each` over `count` for named resources, tag everything, document with comments.
