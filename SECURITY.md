# Security policy

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security vulnerabilities.

Email the maintainer at <contact-email> (replace this placeholder with your actual address before publishing the repo). Include:

- A description of the vulnerability and its impact
- Steps to reproduce (PoC if possible)
- The commit SHA or version where you observed it

You can expect an acknowledgement within 72 hours and a triage decision within 7 days.

## Scope

This template ships with the following defensive scaffolding:

- **Secret scanning** — `.github/workflows/gitleaks.yml` scans every push, every PR, and the full history weekly. `.pre-commit-config.yaml` runs the same scan locally before commit.
- **Dependency review** — `.github/workflows/audit.yml` runs `pnpm audit` / equivalent per workspace and fails on high-severity findings.
- **Static analysis** — `.github/workflows/security.yml` runs CodeQL (SAST) on the configured languages on every PR + weekly.
- **Supply-chain scoring** — `.github/workflows/scorecard.yml` runs OpenSSF Scorecard weekly; results flow to the Security tab and to scorecard.dev.
- **Automated updates** — `.github/dependabot.yml` opens grouped weekly PRs for npm/pip/terraform/GitHub Actions.

## Out of scope

- Issues in dependencies — please report upstream and then optionally let us know.
- Issues that require a malicious maintainer or compromised developer machine.
