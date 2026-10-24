# §7.5: the email substrate. A domain identity with Easy DKIM (three CNAMEs to
# publish), a custom MAIL FROM (an MX plus an SPF TXT), and a configuration set
# so bounces and complaints are tracked rather than discovered as a reputation
# problem.
#
# Transport is the SES **API**, not the SMTP interface. §9.3 budgeted a second
# long-lived IAM access key because SES's SMTP endpoint cannot accept
# credentials derived from temporary ones, and then scheduled that key's
# retirement for Phase 4. Signing SendEmail with the pod's EKS Pod Identity
# credentials takes the exit now, so the key is never created: this module emits
# a policy document (`send_policy_json`) for the caller to attach to a pod role,
# and creates no IAM user and no Secrets Manager entry.
#
# Two facts about SES that are operational, not Terraform's (§7.5): production
# access is a human-reviewed request with a ~24-hour first response, so file it
# early; and sandbox limits (200 messages/day, verified recipients only) are
# fine for dev indefinitely if developers verify their own addresses.
#
# DNS is not managed HERE, and that is about the provider, not about whether
# Terraform owns it. §18.8 resolved the DNS plane to Cloudflare (`dupezero.com`
# at Cloudflare Registrar, which pins the apex to Cloudflare's nameservers —
# there is deliberately no Route 53 zone), so the records are written by the
# `cloudflare` provider in the calling environment rather than by this module.
# `verification_dns`, `mail_from_dns` and `mail_from_domain` are what the caller
# publishes from; see `envs/dev/ses.tf`.
#
# Keeping the records out of this module is what lets it stay single-provider:
# a module that reached for `cloudflare_dns_record` would force every caller to
# configure that provider even when its zone lives somewhere else.

variable "name" {
  description = "Environment-scoped name prefix, e.g. er-dev."
  type        = string
}

variable "domain" {
  description = "The domain to send from, e.g. dev.example.com."
  type        = string
}

variable "mail_from_subdomain" {
  description = <<-EOT
    Subdomain label for the custom MAIL FROM, prefixed to `domain`. A custom
    MAIL FROM is what aligns SPF with the visible From address; without it,
    SES's own domain is what receivers check.
  EOT
  type        = string
  default     = "mail"
}

locals {
  mail_from_domain = "${var.mail_from_subdomain}.${var.domain}"
}

# --- identity ---------------------------------------------------------------

resource "aws_sesv2_email_identity" "this" {
  email_identity = var.domain

  dkim_signing_attributes {
    # Easy DKIM with a 2048-bit key. SES rotates it; the three CNAMEs below
    # stay valid across rotations, which is the reason to prefer this over
    # bringing our own key.
    next_signing_key_length = "RSA_2048_BIT"
  }

  configuration_set_name = aws_sesv2_configuration_set.this.configuration_set_name
}

resource "aws_sesv2_email_identity_mail_from_attributes" "this" {
  email_identity   = aws_sesv2_email_identity.this.email_identity
  mail_from_domain = local.mail_from_domain

  # REJECT_MESSAGE, not USE_DEFAULT_VALUE: if the MAIL FROM records are missing
  # or misconfigured, refuse to send rather than silently falling back to SES's
  # domain, which would break SPF alignment without any signal that it had.
  behavior_on_mx_failure = "REJECT_MESSAGE"
}

# --- configuration set (bounce + complaint tracking) ------------------------

resource "aws_sesv2_configuration_set" "this" {
  configuration_set_name = var.name

  delivery_options {
    # Require TLS on delivery. Transactional mail here carries single-use
    # credentials (invite, reset and signup-verify links), so opportunistic
    # encryption is not good enough.
    tls_policy = "REQUIRE"
  }

  reputation_options {
    reputation_metrics_enabled = true
  }

  sending_options {
    sending_enabled = true
  }

  suppression_options {
    # Account-level suppression for hard bounces and complaints: never retry an
    # address that has already rejected us.
    suppressed_reasons = ["BOUNCE", "COMPLAINT"]
  }
}

# --- the send policy, for a pod role ----------------------------------------

data "aws_iam_policy_document" "send" {
  statement {
    sid    = "SendTransactionalMail"
    effect = "Allow"
    # SendEmail only. No SendBulkEmail (nothing here sends campaigns), no
    # identity or template management (Terraform owns those), and no
    # GetAccount/metrics reads.
    actions = ["ses:SendEmail"]

    # Scoped to this identity, so the role cannot send as any other domain
    # that may later exist in the account...
    resources = [aws_sesv2_email_identity.this.arn]

    # ...and only as an address at this domain. StringLike, not StringEquals:
    # the value is a pattern, and StringEquals would require the From address
    # to be the literal string "*@<domain>".
    condition {
      test     = "StringLike"
      variable = "ses:FromAddress"
      values   = ["*@${var.domain}"]
    }
  }

  statement {
    sid       = "UseTheConfigurationSet"
    effect    = "Allow"
    actions   = ["ses:SendEmail"]
    resources = [aws_sesv2_configuration_set.this.arn]
  }
}

# --- outputs ----------------------------------------------------------------

output "configuration_set_name" {
  description = "Pass to er-server as ER_MAIL_CONFIGURATION_SET."
  value       = aws_sesv2_configuration_set.this.configuration_set_name
}

output "identity_arn" {
  value = aws_sesv2_email_identity.this.arn
}

output "send_policy_json" {
  description = "Attach to the role a Pod Identity association binds to er-api."
  value       = data.aws_iam_policy_document.send.json
}

output "verification_dns" {
  description = <<-EOT
    The three Easy DKIM CNAMEs to publish in Cloudflare. Until they resolve,
    the identity stays unverified and every send fails.
  EOT
  value = [
    for token in aws_sesv2_email_identity.this.dkim_signing_attributes[0].tokens : {
      type  = "CNAME"
      name  = "${token}._domainkey.${var.domain}"
      value = "${token}.dkim.amazonses.com"
    }
  ]
}

output "mail_from_domain" {
  description = <<-EOT
    The custom MAIL FROM subdomain, e.g. `mail.dev.example.com`. Exposed
    because a caller publishing records itself needs the NAME, and because
    Cloudflare's MX resource takes `priority` as its own field — so
    `mail_from_dns`'s "10 feedback-smtp…" string cannot be used verbatim there.
  EOT
  value       = local.mail_from_domain
}

output "mail_from_mx_host" {
  description = <<-EOT
    The MAIL FROM MX target without its priority, for a DNS provider that takes
    priority separately. Priority is 10; `mail_from_dns` carries the combined
    form for anything that wants one string.
  EOT
  value       = "feedback-smtp.${data.aws_region.current.region}.amazonses.com"
}

output "mail_from_spf_value" {
  description = "The SPF TXT value for the MAIL FROM domain."
  value       = "v=spf1 include:amazonses.com ~all"
}

output "mail_from_dns" {
  description = <<-EOT
    The custom MAIL FROM records. With behavior_on_mx_failure = REJECT, sending
    stays broken until these resolve — which is the intended trade: a visible
    failure rather than silent SPF misalignment.
  EOT
  value = [
    {
      type  = "MX"
      name  = local.mail_from_domain
      value = "10 feedback-smtp.${data.aws_region.current.region}.amazonses.com"
    },
    {
      type  = "TXT"
      name  = local.mail_from_domain
      value = "v=spf1 include:amazonses.com ~all"
    },
  ]
}

data "aws_region" "current" {}
