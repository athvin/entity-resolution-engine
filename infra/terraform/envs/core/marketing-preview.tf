# Static assets only, uploaded by marketing-preview.yml. No custom domain,
# functions, databases or production application bindings.
variable "marketing_preview_enabled" {
  description = "Create the static marketing preview host after the core Cloudflare token has Pages Edit permission."
  type        = bool
  default     = false
}

resource "cloudflare_pages_project" "marketing_preview" {
  count = var.marketing_preview_enabled && var.cloudflare_account_id != null ? 1 : 0

  account_id        = var.cloudflare_account_id
  name              = "dupezero-marketing-preview"
  production_branch = "main"
}

output "marketing_preview_hostname" {
  description = "Default Pages hostname; CI reports each deployment's unique URL."
  value       = try(cloudflare_pages_project.marketing_preview[0].subdomain, null)
}
