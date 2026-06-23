# infra-secrets/ — production secrets (sops + AWS KMS)

Encrypted production secrets for *gonedark*, consumed by Terraform (`infra/secrets.tf`).
Local development does **not** use these — it uses the safe defaults in
`.env.development`.

## The one hard rule

**Only KMS-encrypted `*.sops.yaml` files may be committed here.** Never commit a
plaintext secret. `.gitignore` enforces this: everything in this directory is ignored
except `.sops.yaml`, `*.sops.yaml`, `*.example.yaml`, and this README. If `git status`
ever shows a plaintext `prod.yaml`, stop — it's a leak waiting to happen.

Because the values are KMS-encrypted, the committed `*.sops.yaml` is safe even if this
repo is public (only an IAM principal with `kms:Decrypt` on `alias/gonedark-sops` can
read it).

## Create / edit secrets

```
aws sso login --profile gonedark        # once per session
sops infra-secrets/prod.sops.yaml       # opens $EDITOR decrypted; re-encrypts on save
```

`.sops.yaml` in this directory supplies the KMS key and `encrypted_regex`
automatically. Start from the shape in `prod.example.yaml`.

## How Terraform reads it

`infra/secrets.tf` uses the `carlpett/sops` provider:

```hcl
data "sops_file" "prod" { source_file = "${path.module}/../infra-secrets/prod.sops.yaml" }
# local.secrets["database.password"], local.secrets["jwt.signing_key"], ...
```

Alternative (estate convention also allows): decrypt-to-tfvars in CI
(`sops -d prod.sops.yaml > prod.auto.tfvars` immediately before `terraform apply`,
then delete). The provider path is preferred — no plaintext ever hits disk.

## Estate note

Your global convention keeps prod secrets in the separate private
`Absence0760/infra-secrets` repo (one subdir per project), *not* in project repos that
might go public. This project keeps them in-repo per the explicit `./infra-secrets`
instruction; the encryption makes that safe (same pattern meryl-green-designs uses
today). If this repo is ever published and you'd rather not ship the ciphertext, lift
this directory into the private repo and repoint `infra/secrets.tf` — nothing else
changes. Pattern reference: `~/github/templates/docs/secrets-management.md`.
