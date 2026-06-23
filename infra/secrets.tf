# Production secrets, decrypted at plan/apply time from the KMS-encrypted file in
# ../infra-secrets. Nothing is ever written to disk in plaintext; values stay in
# Terraform state, so the state bucket itself is treated as sensitive.
#
# This is the "carlpett/sops provider" path from the estate convention. The
# alternative (decrypt-to-tfvars in CI) is noted in docs/infrastructure.md.

data "sops_file" "prod" {
  source_file = "${path.module}/../infra-secrets/prod.sops.yaml"
}

locals {
  # Nested YAML is flattened to dotted keys, e.g. local.secrets["database.password"].
  secrets = data.sops_file.prod.data
}

# Example consumption (uncomment once real resources exist):
#
# resource "aws_db_instance" "game" {
#   username = local.secrets["database.username"]
#   password = local.secrets["database.password"]
#   # ...
# }
