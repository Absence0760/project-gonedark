terraform {
  required_version = ">= 1.15.0" # pinned via tfenv (.terraform-version)

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
    # carlpett/sops — reads KMS-encrypted secrets from ../infra-secrets at plan time.
    sops = {
      source  = "carlpett/sops"
      version = "~> 1.0"
    }
  }

  # Remote state in this project's own AWS account (S3 + DynamoDB lock), created by
  # the estate baseline (`new-project-account.sh gonedark`). Fill in after bootstrap.
  backend "s3" {
    # bucket         = "gonedark-tfstate-<account-id>"
    # key            = "infra/terraform.tfstate"
    # region         = "us-east-1"
    # dynamodb_table = "gonedark-tflock"
    # profile        = "gonedark"   # SSO profile
    # encrypt        = true
  }
}
