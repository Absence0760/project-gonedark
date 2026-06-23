# Production infrastructure for the gonedark backend (matchmaker, relay, accounts,
# telemetry). Empty for now — pre-production has no deployable services yet.
#
# When the backend exists, resources go here (or in split files: networking.tf,
# database.tf, compute.tf, ...). They consume secrets via local.secrets (secrets.tf)
# and the conventions in this directory. Budgets/alarms and ACM certs live in this
# project's Terraform per the estate convention, not the account baseline.
