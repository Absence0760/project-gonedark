variable "aws_region" {
  description = "AWS region for this project's resources."
  type        = string
  default     = "us-east-1"
}

variable "aws_profile" {
  description = "Local AWS SSO profile for this project's account (see ~/CLAUDE.md estate notes)."
  type        = string
  default     = "gonedark"
}

variable "environment" {
  description = "Deployment environment name (prod, staging, ...)."
  type        = string
  default     = "prod"
}
