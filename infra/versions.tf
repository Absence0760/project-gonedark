terraform {
  required_version = ">= 1.15.0" # pinned via tfenv (.terraform-version)

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.51"
    }
    # carlpett/sops — reads KMS-encrypted secrets from the private estate repo
    # (../../infra-secrets/gonedark, sibling of this repo) at plan time. See D12.
    sops = {
      source  = "carlpett/sops"
      version = "~> 1.0"
    }
  }

  # Remote state in this project's own AWS account (S3 + DynamoDB lock), created by
  # the estate baseline (`new-project-account.sh gonedark`). Fill in + UNCOMMENT the whole
  # block after bootstrap. Left fully commented so `terraform validate` passes on the
  # un-bootstrapped scaffold (CI runs `fmt -check` + `validate`): an empty `backend "s3" {}`
  # with no bucket/key is invalid config and would fail validate. Until then state is local.
  # backend "s3" {
  #   bucket         = "gonedark-tfstate-<account-id>"
  #   key            = "infra/terraform.tfstate"
  #   region         = "us-east-1"
  #   dynamodb_table = "gonedark-tflock"
  #   profile        = "gonedark"   # SSO profile
  #   encrypt        = true
  # }
}
