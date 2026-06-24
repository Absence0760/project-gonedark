# infra/ — Terraform

Production infrastructure for the *gonedark* backend, in this project's own AWS
account (part of the personal org). Local development needs **none** of this — use
`docker compose` instead (see [`../docs/infrastructure.md`](../docs/infrastructure.md)).

## One-time bootstrap (per the estate convention)

This project's AWS account + lean baseline (tfstate S3 + DynamoDB lock, `gonedark-sops`
KMS key, GitHub OIDC + scoped deploy role, delegated `gonedark.jaredhoward.com` zone)
is created by the estate tooling:

```
~/github/templates/scripts/new-project-account.sh gonedark
```

Then fill in the `backend "s3"` block in `versions.tf` with the bucket / lock table /
SSO profile that script provisions.

## Usage

```
tfenv install            # honors .terraform-version (1.15.0)
terraform init
terraform plan
terraform apply
```

`plan`/`apply` decrypt `../../infra-secrets/gonedark/prod.sops.yaml` (in the separate
private estate repo, a sibling of this one under `~/github/` — see D12) via the
`carlpett/sops` provider, using your AWS SSO credentials (`kms:Decrypt`). Run `aws sso
login --profile gonedark` first if your session has expired.

## Files

| File | Purpose |
|---|---|
| `versions.tf` | Terraform + provider versions, S3 remote-state backend |
| `providers.tf` | AWS provider (SSO profile), default tags |
| `variables.tf` | region / profile / environment |
| `secrets.tf` | `data "sops_file"` → `local.secrets` (reads `../../infra-secrets/gonedark/`) |
| `main.tf` | backend resources (empty until the server side exists) |

## Notes

- Secrets never hit disk in plaintext, but they **do** land in Terraform state — treat
  the state bucket as sensitive (it's encrypted + access-controlled by the baseline).
- Budgets/alarms and ACM certs belong here, not in the account baseline.
