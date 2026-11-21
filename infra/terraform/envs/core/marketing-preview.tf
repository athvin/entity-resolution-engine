# Static assets only, uploaded by marketing-preview.yml. No custom domain,
# functions, databases or production application bindings.
variable "marketing_preview_enabled" {
  description = "Create the static marketing preview host using a dedicated Pages credential."
  type        = bool
  default     = false
}

# Ephemeral provider credentials are omitted from both plan files and state.
# Keep Pages separate from the existing R2 token: neither needs the other's scope.
variable "cloudflare_pages_api_token" {
  description = "Dedicated Pages credential, supplied only to core apply and drift jobs."
  type        = string
  sensitive   = true
  ephemeral   = true
  default     = null
}

provider "cloudflare" {
  alias     = "pages"
  api_token = var.cloudflare_pages_api_token
}

resource "cloudflare_pages_project" "marketing_preview" {
  provider = cloudflare.pages
  count    = var.marketing_preview_enabled && var.cloudflare_account_id != null ? 1 : 0

  account_id        = var.cloudflare_account_id
  name              = "dupezero-marketing-preview"
  production_branch = "main"
}

output "marketing_preview_hostname" {
  description = "Default Pages hostname; CI reports each deployment's unique URL."
  value       = try(cloudflare_pages_project.marketing_preview[0].subdomain, null)
}
