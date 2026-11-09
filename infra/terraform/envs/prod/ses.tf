# §7.5/§9.3: prod's email substrate — the identity er-api signs SendEmail as,
# and the first policy the er-prod-api-pod role has ever carried.
#
# The mirror of envs/dev/ses.tf with dev's opt-in clause removed. Dev is
# opt-in because its plan tier had to stay plannable for a hundred unrelated
# resources without CLOUDFLARE_API_TOKEN; prod settled that differently in
# edge.tf — the zone id is a defaulted VARIABLE there, the token reaches only
# the apply tier (scripts/ci/tf-cloudflare-token.sh), and a `-refresh=false`
# plan over cloudflare_dns_record RESOURCES needs no token (edge.tf's header
# records the measurement). So this file is always on and the plan tier stays
# credential-free.
#
# Prod sends from the APEX, not a subdomain. var.edge_domain is the variable
# the certificate and the site records already use, and
# frontend/src/lib/site/routes.ts hardcoding CANONICAL_ORIGIN is what decided
# that the app and its mail share one domain.
#
# Nothing here mints a static credential (§9.3): er-server calls the SES API
# and signs with the pod's Pod Identity credentials, so the SMTP interface's
# long-lived-key problem never arises.
#
# Two things are deliberately NOT here:
#
#   * The values-prod.yaml backend flip (#943). SES stays in the sandbox until
#     the production-access request is granted — #939's owner action, a
#     multi-day AWS lead. A sandbox send to an unverified recipient is a
#     MessageRejected, which LOSES the link the log backend currently returns
#     in the response, so the flip is gated on the grant.
#
#   * A DMARC record. None exists for the apex (checked 2026-11-09:
#     `_dmarc.dupezero.com` answers nothing). The policy choice and where
#     aggregate reports go are deliverability decisions, raised in #943 rather
#     than guessed at here.

module "ses" {
  source = "../../modules/ses"

  name   = "er-prod"
  domain = var.edge_domain
}

# --- the records §18.8 has Terraform write ----------------------------------
#
# Same three-record shape as dev: three Easy DKIM CNAMEs, the MAIL FROM MX,
# and the MAIL FROM SPF TXT. All DNS-only — a proxied CNAME would not serve
# the DKIM token at all.

# `count = 3`, not `for_each` over the output: the DKIM tokens are computed,
# and Terraform requires for_each keys at PLAN time. Easy DKIM always returns
# exactly three tokens, so a fixed count is both correct and plannable.
resource "cloudflare_dns_record" "ses_dkim" {
  count = 3

  zone_id = var.cloudflare_zone_id
  name    = module.ses.verification_dns[count.index].name
  type    = "CNAME"
  content = module.ses.verification_dns[count.index].value
  ttl     = 600
  proxied = false
}

# With `behavior_on_mx_failure = REJECT` in the module, sending stays broken
# until this resolves — the intended trade: a visible failure rather than
# silent SPF misalignment.
resource "cloudflare_dns_record" "ses_mail_from_mx" {
  zone_id  = var.cloudflare_zone_id
  name     = module.ses.mail_from_domain
  type     = "MX"
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

# Attached to the shared er-api pod role (pod_identity.tf), not to a role of
# its own: Pod Identity binds one role per (cluster, namespace, ServiceAccount)
# triple, so mail is a policy on the pod's one role. This is that role's FIRST
# policy — the "deliberately carries NO policy yet" note pod_identity.tf was
# born with predates this file.
resource "aws_iam_role_policy" "mail" {
  name   = "send-transactional-mail"
  role   = aws_iam_role.er_api_pod.id
  policy = module.ses.send_policy_json
}

output "ses_configuration_set_name" {
  description = "er-server's ER_MAIL_CONFIGURATION_SET — the value #943's values-prod.yaml flip needs."
  value       = module.ses.configuration_set_name
}

output "ses_identity_arn" {
  description = "The apex identity. Verification state reads from this; sandbox/production state is account-level."
  value       = module.ses.identity_arn
}
