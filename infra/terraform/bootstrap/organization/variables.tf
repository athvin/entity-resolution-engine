# Supplied via terraform.tfvars, which is git-ignored: the repository is
# public and account root emails do not belong in it.

variable "nonprod_root_email" {
  description = "Root email of the parked former-nonprod member account (permanent)."
  type        = string

  validation {
    condition     = can(regex("@", var.nonprod_root_email))
    error_message = "Must be an email address."
  }
}
