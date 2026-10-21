# Bedrock: the AI substrate, and the two halves of enabling it.
#
# The halves are worth separating because only one of them is a decision.
#
#   1. PERMISSION — the IAM policy letting the `er-api` pod invoke Claude, and
#      the Pod Identity binding that delivers it. No cost, no agreement, nothing
#      outward-facing; applied as soon as a namespace is listed.
#
#   2. MODEL ACCESS — the account's acceptance of Anthropic's terms on Bedrock.
#      `aws_bedrock_use_case_for_model_access` submits a customer-profile form to
#      AWS (company name, website, intended use) and
#      `aws_bedrock_foundation_model_agreement` accepts a PRICED offer. Both are
#      the account owner's to make, so both are gated on
#      `bedrock_use_case_form` being supplied. Until then the permission half
#      still applies cleanly and `converse` answers `Operation not allowed`,
#      which is what `ER_AI_*` unset already degrades to.
#
# Probed 2026-10-21 in us-east-2: the Claude inference profiles exist and are
# ACTIVE with `entitlementAvailability: AVAILABLE`, and
# `GetUseCaseForModelAccess` answers "You have not filled out the request form"
# — i.e. (1) is all that is missing on our side, and (2) is all that is missing
# on AWS's.

variable "bedrock_inference_profile_prefixes" {
  description = <<-EOT
    Inference-profile id prefixes the pod may invoke, as IAM resource patterns.

    Cross-region profiles (`us.*`, `global.*`) are the only way to reach the
    current Claude models — there is no single-region endpoint for them — so the
    profile is what gets invoked, not the bare model.
  EOT
  type        = list(string)
  default     = ["us.anthropic.claude-*", "global.anthropic.claude-*"]
}

variable "bedrock_use_case_form" {
  description = <<-EOT
    The customer-profile form AWS requires before Anthropic models can be
    enabled, as the raw JSON body of `PutUseCaseForModelAccess` (the provider
    passes it through as `form_data`).

    Deliberately `null` by default, and NOT something this repo can fill in: it
    submits a legal entity's name, website and intended use to AWS, and the
    agreement it unlocks is usage-priced (as of 2026-10-21, Claude Sonnet 4.6 in
    us-east-2 lists $3/M input and $15/M output tokens). Supplying it is the
    account owner's act.

    When set, the agreement resources below are created and Claude becomes
    invokable. When null, everything else here still applies.
  EOT
  type        = string
  default     = null
  sensitive   = true
}

variable "bedrock_agreement_model_ids" {
  description = <<-EOT
    The foundation models to accept agreements for. Only consulted when
    `bedrock_use_case_form` is set.
  EOT
  type        = list(string)
  default     = ["anthropic.claude-sonnet-4-6", "anthropic.claude-haiku-4-5-20251001-v1:0"]
}

# ---------------------------------------------------------------------------- #
# 1. Permission — no decision required
# ---------------------------------------------------------------------------- #

data "aws_caller_identity" "current" {}

# The region is pinned in the provider block (versions.tf), not a variable.
data "aws_region" "current" {}

data "aws_iam_policy_document" "bedrock_invoke" {
  # The profile is what the application names when it calls Converse.
  statement {
    sid    = "InvokeClaudeInferenceProfiles"
    effect = "Allow"

    actions = [
      "bedrock:InvokeModel",
      "bedrock:InvokeModelWithResponseStream",
      "bedrock:Converse",
      "bedrock:ConverseStream",
    ]

    resources = [
      for prefix in var.bedrock_inference_profile_prefixes :
      "arn:aws:bedrock:${data.aws_region.current.region}:${data.aws_caller_identity.current.account_id}:inference-profile/${prefix}"
    ]
  }

  # A cross-region profile routes the request to whichever region has capacity,
  # and Bedrock authorizes the FOUNDATION MODEL in the region it lands in — so
  # this statement cannot be pinned to one region without the call failing
  # exactly when the profile does its job. The region wildcard is scoped to
  # Anthropic models in return, which is the trade worth making.
  statement {
    sid    = "InvokeClaudeFoundationModelsWhereverTheProfileRoutes"
    effect = "Allow"

    actions = [
      "bedrock:InvokeModel",
      "bedrock:InvokeModelWithResponseStream",
    ]

    resources = ["arn:aws:bedrock:*::foundation-model/anthropic.claude-*"]
  }

  # Read-only discovery, so the application can resolve a model id rather than
  # carry a hardcoded one.
  statement {
    sid       = "DiscoverModels"
    effect    = "Allow"
    actions   = ["bedrock:ListFoundationModels", "bedrock:GetFoundationModel"]
    resources = ["*"]
  }
}

resource "aws_iam_role_policy" "bedrock_invoke" {
  name   = "invoke-claude"
  role   = aws_iam_role.er_api_pod.id
  policy = data.aws_iam_policy_document.bedrock_invoke.json
}

# ---------------------------------------------------------------------------- #
# 2. Model access — the account owner's decision
# ---------------------------------------------------------------------------- #

locals {
  # Whether the form was supplied is not secret; its contents are. Terraform
  # propagates the variable's sensitivity to anything derived from it, and a
  # sensitive value cannot drive `count`/`for_each` (it would surface as a
  # resource instance key), so the gate is unwrapped once, here.
  bedrock_model_access = nonsensitive(var.bedrock_use_case_form != null)
}

resource "aws_bedrock_use_case_for_model_access" "this" {
  count = local.bedrock_model_access ? 1 : 0

  form_data = var.bedrock_use_case_form
}

# The offer token is read rather than hardcoded: it is a signed blob that
# encodes the current rate card, and a stale one would accept terms that are no
# longer the terms.
data "aws_bedrock_foundation_model_agreement_offers" "claude" {
  for_each = local.bedrock_model_access ? toset(var.bedrock_agreement_model_ids) : toset([])

  model_id = each.value
}

resource "aws_bedrock_foundation_model_agreement" "claude" {
  for_each = local.bedrock_model_access ? toset(var.bedrock_agreement_model_ids) : toset([])

  model_id    = each.value
  offer_token = data.aws_bedrock_foundation_model_agreement_offers.claude[each.key].offers[0].offer_token

  # The form has to be on file before an agreement will be accepted; without
  # this, a first apply races and fails with the "fill out the form" error.
  depends_on = [aws_bedrock_use_case_for_model_access.this]
}

output "bedrock_model_access_enabled" {
  description = "Whether this env has accepted the Anthropic model agreements."
  value       = local.bedrock_model_access
}
