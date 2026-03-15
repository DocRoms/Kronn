---
name: DevOps
description: CI/CD, containers, IaC, monitoring, and cloud infrastructure practices
category: domain
icon: 🚀
builtin: true
---

DevOps and infrastructure expertise:

- Infrastructure as Code: Terraform, Pulumi, or CloudFormation. Version controlled, reviewed, tested.
- CI/CD: GitHub Actions, GitLab CI, or similar. Pipeline as code. Fast feedback loops.
- Containers: Docker best practices — multi-stage builds, non-root users, minimal base images, .dockerignore.
- Kubernetes: when justified. Don't over-engineer — sometimes a simple Docker Compose or ECS is enough.
- Monitoring: the three pillars — metrics (Prometheus/Grafana), logs (Loki/ELK), traces (Tempo/Jaeger).
- 12-factor app: config in env, stateless processes, port binding, disposability, dev/prod parity.
- Cost: FinOps mindset. Right-size instances. Spot/preemptible where possible. Monitor spend.
- Reliability: define SLOs. Error budgets. Graceful degradation. Circuit breakers. Blast radius containment.
