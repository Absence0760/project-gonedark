# Infrastructure, local dev & secrets

How the project runs locally (clone-and-go) and how production infra and secrets are
managed. The game **client** is a native Rust app; this doc is about the **backend/
services** side it will eventually talk to (matchmaker, relay, accounts, telemetry,
and cosmetic store/entitlements — see [`decisions.md`](decisions.md) D13,
[`open-questions.md`](open-questions.md) Q9) and
the cloud infra behind it. Most of this is scaffolding ahead of the server code — the
conventions are fixed now so nothing has to be retrofitted.

## Principle

- **Local development is fully self-contained and needs no cloud access.** A fresh
  clone runs immediately against local Docker services using committed, non-secret
  defaults. No AWS login, no real secrets, no manual config to start.
- **Production secrets are KMS-encrypted (sops) and consumed by Terraform.** They never
  appear in plaintext in the repo, in `.env` files, or on disk during deploys.
- **All cloud infrastructure is Terraform.** No click-ops.

## Quickstart (new contributor)

```
git clone <repo> && cd project-gonedark
docker compose up -d        # Postgres + Redis, defaults from compose.yaml
cargo run                   # app loads .env.development   (once engine code exists)
```

That's it — no secrets to fetch, nothing to configure.

## Configuration & env files

| File | Committed? | Purpose |
|---|---|---|
| `.env.development` | **yes** | Safe local defaults. Clone-and-run. Assume everything in it is public. |
| `.env.local` | no (gitignored) | Personal overrides (e.g. a different port). Takes precedence. |
| `../../infra-secrets/gonedark/*.sops.yaml` | yes, but in a **separate private repo** (encrypted) | **Production** secrets only. Not in this repo; never read locally. |

Precedence: `.env.local` > `.env.development`. Never put a real secret in either — real
values live only in the private estate repo (`~/github/infra-secrets/gonedark/`, prod)
and are injected by Terraform/deploy, not by `.env`.

## Local services (Docker)

`compose.yaml` brings up the backend's runtime dependencies:

- **Postgres 17** — accounts, leaderboards, replay/validation records. Host port
  **5434** (avoids this workstation's Supabase 5432 and native 5433).
- **Redis 7** — ephemeral session/matchmaking state; persistence off for dev.

```
docker compose up -d     # start          docker compose ps      # status
docker compose logs -f   # tail           docker compose down -v # stop + wipe data
```

Defaults are inlined in `compose.yaml` (`${VAR:-default}`), so it works with zero env
setup; `.env.development` carries the matching values for the app.

## Production infrastructure (Terraform)

Lives in [`infra/`](../infra/). This project's AWS account and lean baseline (tfstate
S3 + lock, `gonedark-sops` KMS key, GitHub OIDC deploy role, delegated
`gonedark.jaredhoward.com` zone) come from the estate tooling
(`new-project-account.sh gonedark`); then `terraform init/plan/apply`. See
[`infra/README.md`](../infra/README.md) for the full loop. tfenv pins Terraform 1.15.0.

## Secrets (sops + AWS KMS)

Secrets do **not** live in this repo. They sit in the separate private estate repo,
`~/github/infra-secrets/gonedark/` (a sibling of this repo, one subdir per project) —
KMS-encrypted via your SSO profile (no local age key). **Only encrypted `*.sops.yaml`
is ever committed**, and only to that private repo, so this potentially-public game
repo never ships ciphertext (decision D12). Terraform reads them through the
`carlpett/sops` provider (`data "sops_file"` at `../../infra-secrets/gonedark/prod.sops.yaml`
→ `local.secrets[...]`). Create/edit with `sops gonedark/prod.sops.yaml` from inside
that repo (after `aws sso login --profile gonedark`). Full per-project workflow in the
estate repo's `README.md`; estate pattern reference in
`~/github/templates/docs/secrets-management.md`.

## Why this shape

- **Clone-and-run lowers the cost of every future contributor** (including AI agents
  driving the build loop) — no credential dance to see the thing work.
- **Encrypted-by-default secrets + Terraform-only infra** means the prod path is
  auditable, reproducible, and safe to keep in git, matching the rest of the estate.
