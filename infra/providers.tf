provider "aws" {
  region  = var.aws_region
  profile = var.aws_profile # SSO profile for this project's account

  default_tags {
    tags = {
      Project   = "gonedark"
      ManagedBy = "terraform"
      Env       = var.environment
    }
  }
}

# carlpett/sops needs no configuration block — decryption is driven by the KMS key
# referenced in the encrypted file's metadata and the caller's AWS credentials
# (kms:Decrypt via the SSO profile above). See secrets.tf.
