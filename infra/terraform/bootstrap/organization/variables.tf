# Root emails are supplied via terraform.tfvars, which is git-ignored: the
# repository is public and account root emails do not belong in it.

variable "nonprod_root_email" {
  description = "Root email for the nonprod member account (permanent once used)."
  type        = string

  validation {
    condition     = can(regex("@", var.nonprod_root_email))
    error_message = "Must be an email address."
  }
}

variable "prod_root_email" {
  description = "Root email for the prod member account (permanent once used)."
  type        = string

  validation {
    condition     = can(regex("@", var.prod_root_email))
    error_message = "Must be an email address."
  }
}
