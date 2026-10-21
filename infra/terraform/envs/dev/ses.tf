# §7.5/§9.3: dev's email substrate, and the role er-api signs SendEmail with.
#
# Nothing here mints a static credential. §9.3's second IAM access key existed
# only because SES's SMTP interface cannot accept temporary credentials;
# er-server now calls the SES API, so the pod's Pod Identity credentials sign
# it and the key is never created. That leaves the §9.2 S3 user (lake.tf) as
# the account's one remaining long-lived key.
#
# Two things stay manual and block real delivery until done (§7.5):
#   1. Publish `ses_verification_dns` and `ses_mail_from_dns` in Cloudflare.
#      Until the DKIM CNAMEs resolve the identity is unverified; until the MAIL
#      FROM records resolve, behavior_on_mx_failure = REJECT keeps sends
#      failing. Both are deliberate: a visible failure, not silent misalignment.
#   2. File the SES production-access request (~24h first response). Until it
#      is granted the account is in the sandbox — 200 messages/day, and only to
#      verified recipients. That is fine for dev indefinitely if each developer
#      verifies their own address.
#
# Until both land, leave ER_MAIL_BACKEND unset or `log` and the verify/invite
# links appear in the server log. er-server degrades to 503 on every send with
# no backend, which is what the BFF's relay already treats as `false`.

module "ses" {
  source = "../../modules/ses"

  name   = "er-dev"
  domain = var.ses_domain
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
    The domain dev sends from. Must be a domain whose DNS is ours to publish
    (Cloudflare, §18.8) — SES verification needs three CNAMEs under it.
  EOT
  type        = string
}

output "ses_configuration_set_name" {
  description = "er-server's ER_MAIL_CONFIGURATION_SET."
  value       = module.ses.configuration_set_name
}

output "ses_verification_dns" {
  description = "Publish these three CNAMEs in Cloudflare to verify the identity."
  value       = module.ses.verification_dns
}

output "ses_mail_from_dns" {
  description = "Publish these to align SPF with the visible From address."
  value       = module.ses.mail_from_dns
}
