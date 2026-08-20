# Security policy

Going Dark is a pre-release game engine. There is no deployed service, no user
accounts, and no user data — so the realistic attack surface is the repo and the
build pipeline, not a production system.

## Reporting a vulnerability

**Do not open a public issue.** Use GitHub's private vulnerability reporting:
**Security → Advisories → Report a vulnerability** on this repository.

Include the commit SHA, what you observed, and a reproduction if you have one.
This is a personal project maintained on a best-effort basis — expect an
acknowledgement within a week, not an SLA.

## What's in scope

- The engine and server crates in this workspace (`core/ pal/ render/ engine/
  pal-desktop/ pal-android/ app/ server/` …).
- The Terraform in `infra/` and the GitHub Actions workflows in `.github/`.
- A secret committed to this repo (see below — there should never be one).

## What's not in scope

- Vulnerabilities in third-party crates. Report those upstream; `cargo-deny`
  (`.github/workflows/audit.yml`) already fails CI on a RustSec advisory here.
- Anything requiring a compromised maintainer machine.
- Game-balance exploits and desyncs — those are bugs, file them as issues.

## Defensive scaffolding in this repo

| Control | Where |
|---|---|
| Secret scanning (push, PR, weekly full-history sweep) | `.github/workflows/gitleaks.yml` + `.gitleaks.toml` |
| Rust dependency advisories + license/source bans | `.github/workflows/audit.yml` + `deny.toml` |
| Static analysis (CodeQL: GitHub Actions + Python) | `.github/workflows/security.yml` |
| Supply-chain posture score (weekly, advisory) | `.github/workflows/scorecard.yml` |
| Grouped dependency updates | `.github/dependabot.yml` |
| Cross-platform determinism / lockstep gate | `.github/workflows/determinism.yml` |

## Secrets

Per invariant #8 in `CLAUDE.md`: **no secret of any kind belongs in this repo.**
Local development runs against Docker with non-secret defaults in
`.env.development`. Real secrets are KMS-encrypted with sops in a separate
private estate repo and read by Terraform through the `carlpett/sops` provider.
If you find a plaintext credential in the history, report it privately as above
rather than opening a PR that deletes it.
