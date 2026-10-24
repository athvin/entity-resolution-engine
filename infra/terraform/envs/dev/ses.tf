# §7.5/§9.3: dev's email substrate, and the role er-api signs SendEmail with.
#
# Nothing here mints a static credential. §9.3's second IAM access key existed
# only because SES's SMTP interface cannot accept temporary credentials;
# er-server now calls the SES API, so the pod's Pod Identity credentials sign
# it and the key is never created. That leaves the §9.2 S3 user (lake.tf) as
# the account's one remaining long-lived key.
#
# The DNS half is Terraform's now. This file used to list "publish these
# records in Cloudflare" as a manual step, which §18.8 never intended: it
# resolved the DNS plane to Cloudflare AND named the Terraform `cloudflare`
# provider as what writes into it — "SES Easy-DKIM CNAMEs and MAIL FROM records
# land in the Cloudflare zone the same way" as the ACM validation CNAME. The
# records below are that, so an apply leaves nothing to paste.
#
# There is deliberately no Route 53 zone (§18.8): Cloudflare Registrar pins the
# apex to Cloudflare's nameservers, and a second zone would be a second DNS
# authority for one domain.
#
# What stays manual, because it is genuinely not Terraform's:
#   * **SES production access** — a human-reviewed request with a ~24h first
#     response. Until granted the account is in the sandbox: 200 messages/day
#     and only to VERIFIED recipients. Fine for dev indefinitely if each
#     developer verifies their own address, and the reason `ER_RUN_REPORT_TO`
#     absent is the opt-out — a tenant whose address is not verified gets
#     nothing rather than a bounce loop.
#
# Running a plan over this needs `CLOUDFLARE_API_TOKEN` exported (versions.tf).
#
# Until both land, leave ER_MAIL_BACKEND unset or `log` and the verify/invite
# links appear in the server log. er-server degrades to 503 on every send with
# no backend, which is what the BFF's relay already treats as `false`.

module "ses" {
  source = "../../modules/ses"

  name   = "er-dev"
  domain = var.ses_domain
}

# --- the records §18.8 said Terraform writes -------------------------------

# Easy DKIM: three CNAMEs. Until they resolve the identity stays unverified and
# every send fails, so these are the difference between an apply and a working
# sender.
#
# `count = 3`, not `for_each` over the output: the DKIM tokens are computed, and
# Terraform requires for_each keys to be known at PLAN time — `for_each` over
# them fails with "the for_each value depends on resource attributes that cannot
# be determined until apply" before anything is created. Easy DKIM always
# returns exactly three tokens, so a fixed count is both correct and plannable;
# indexing a computed list is fine because only the values are unknown.
resource "cloudflare_dns_record" "ses_dkim" {
  count = 3

  zone_id = var.cloudflare_zone_id
  name    = module.ses.verification_dns[count.index].name
  type    = "CNAME"
  content = module.ses.verification_dns[count.index].value
  ttl     = 600
  # DNS-only, never proxied. §18.8 is explicit about this for dev hostnames —
  # they resolve to an internal, tailnet-reached ALB Cloudflare's proxy could
  # never reach — and a proxied DKIM CNAME would not serve the token at all.
  proxied = false
}

# The custom MAIL FROM MX. With `behavior_on_mx_failure = REJECT` in the module,
# sending stays broken until this resolves — the intended trade: a visible
# failure rather than silent SPF misalignment.
resource "cloudflare_dns_record" "ses_mail_from_mx" {
  zone_id = var.cloudflare_zone_id
  name    = module.ses.mail_from_domain
  type    = "MX"
  # Cloudflare takes priority as its own field, which is why the module exposes
  # the host separately from `mail_from_dns`'s combined "10 feedback-smtp…".
  content  = module.ses.mail_from_mx_host
  priority = 10
  ttl      = 600
}

# SPF for the MAIL FROM domain, which is what aligns SPF with the visible From
# address. Without it receivers check SES's own domain instead.
resource "cloudflare_dns_record" "ses_mail_from_spf" {
  zone_id = var.cloudflare_zone_id
  name    = module.ses.mail_from_domain
  type    = "TXT"
  content = module.ses.mail_from_spf_value
  ttl     = 600
}

# Attached to the SHARED er-api pod role (pod_identity.tf), not to a role of
# its own. The web+api Deployment has one ServiceAccount, and Pod Identity
# binds one role per (cluster, namespace, service account) — so mail and
# Bedrock have to be two policies on one role, not two roles.
resource "aws_iam_role_policy" "mail" {
  name   = "send-transactional-mail"
  role   = aws_iam_role.er_api_pod.id
  policy = module.ses.send_policy_json
}

variable "ses_domain" {
  description = <<-EOT
    The domain dev sends from, e.g. `dev.dupezero.com`. Must sit inside the
    Cloudflare zone named by `cloudflare_zone_id`, since that is where the
    verification and MAIL FROM records are written.
  EOT
  type        = string
}

variable "cloudflare_zone_id" {
  description = <<-EOT
    The Cloudflare zone holding `ses_domain` — `dupezero.com`'s zone id
    (§18.8). Not a secret: it is an opaque identifier, and the credential that
    can act on it is `CLOUDFLARE_API_TOKEN` in the environment.
  EOT
  type        = string
}

output "ses_configuration_set_name" {
  description = "er-server's ER_MAIL_CONFIGURATION_SET."
  value       = module.ses.configuration_set_name
}

output "ses_verification_dns" {
  description = <<-EOT
    The three Easy DKIM CNAMEs. INFORMATIONAL — `cloudflare_dns_record.ses_dkim`
    publishes them. Kept for reading a plan and for debugging a verification
    that has not completed.
  EOT
  value       = module.ses.verification_dns
}

output "ses_mail_from_dns" {
  description = <<-EOT
    The MAIL FROM records, in combined single-string form. INFORMATIONAL — the
    `ses_mail_from_mx` and `ses_mail_from_spf` resources publish them, using the
    module's split outputs because Cloudflare takes MX priority separately.
  EOT
  value       = module.ses.mail_from_dns
}
