# The §4.1 prod edge: internet-facing ALB, behind AWS WAF, fronted by DNS-only
# Cloudflare records.
#
# §4.1's reasoning, unchanged: dev is an internal ALB over Tailscale because §13
# fills developer namespaces with real production records. Prod customers arrive
# from the internet, so the boundary moves from network to WAF plus application
# auth, and the non-negotiable controls are rate-based.
#
# --- why DNS-only rather than Cloudflare-proxied ------------------------------
#
# Operator decision 2026-11-08. The zone stays grey-cloud and AWS WAF does the
# filtering, for three reasons:
#
#   1. A proxied edge is bypassable unless the ALB's security group is pinned to
#      Cloudflare's published IP ranges, and that list needs refreshing — a
#      maintenance task whose failure mode is silent and wide open.
#   2. The zone is on Cloudflare's Free plan (verified 2026-11-08), which gives
#      one rate-limiting rule. §4.1 needs three.
#   3. WAF on the ALB has no bypass path at all: there is no route to the
#      targets that does not traverse the listener the web ACL is bound to.
#
# --- why there is no external-dns ---------------------------------------------
#
# The chart carries an `ingress-web.yaml` written for dev, with external-dns
# annotations and `scheme: internal`. Prod does not use it. Terraform owns the
# load balancer here, so Terraform already knows its DNS name and can write the
# record itself — which removes a controller, the scoped Cloudflare token it
# would have needed, and the ExternalSecret to project that token. The pods join
# this load balancer through a `TargetGroupBinding` instead (#556).
#
# --- the constraint this file must respect ------------------------------------
#
# NO CLOUDFLARE `data` SOURCES IN THIS STACK. The plan tier runs credential-free
# (§14.2: `er-ci-tf-plan` is assumable from any PR branch, so it is given no
# secrets), and a `-refresh=false` plan over Cloudflare RESOURCES needs no token
# — verified 2026-11-08 by planning a `cloudflare_dns_record` with no
# credentials in the environment: `Plan: 1 to add`. A `data` source is read at
# plan time regardless of `-refresh=false`, so adding one would break the
# credential-free tier. Look values up with a variable instead.

variable "edge_domain" {
  description = <<-EOT
    The apex this environment serves.

    Not a free choice: `frontend/src/lib/site/routes.ts` hardcodes
    `CANONICAL_ORIGIN = "https://dupezero.com"`, and the marketing pages and the
    application are the same Next.js app — so prod is the apex, serving `/`,
    `/login`, `/app` and `/api/*` from one hostname.
  EOT
  type        = string
  default     = "dupezero.com"
}

variable "cloudflare_zone_id" {
  description = <<-EOT
    The Cloudflare zone holding `edge_domain`.

    A zone id is a public identifier, not a credential, so it is defaulted here
    rather than passed in. Verified 2026-11-08: this id resolves to
    `dupezero.com`, status active. The credential is `CLOUDFLARE_API_TOKEN` in
    the environment, which only the apply tier is given.
  EOT
  type        = string
  default     = "2f036e2791d0b695a23145e5f4ae1fb7"
}

# --- TLS ----------------------------------------------------------------------

resource "aws_acm_certificate" "edge" {
  domain_name               = var.edge_domain
  subject_alternative_names = ["www.${var.edge_domain}"]
  validation_method         = "DNS"

  # The certificate is referenced by a listener, so a replacement has to exist
  # before the old one goes away or the listener is briefly certificate-less.
  lifecycle {
    create_before_destroy = true
  }

  tags = {
    Name = "er-prod-edge"
  }
}

# ACM's validation records, written into Cloudflare.
#
# Keyed by `domain_name` rather than by index: ACM emits one option per name,
# and where two names share a validation record (an apex plus its wildcard, for
# instance) this collapses them instead of declaring the same record twice.
# `trimsuffix` because ACM returns fully-qualified names with a trailing dot.
#
# These records must SURVIVE. ACM re-validates through them at renewal, so
# deleting them after the first issue breaks the certificate 13 months later,
# long after anyone connects the two events.
resource "cloudflare_dns_record" "acm_validation" {
  for_each = {
    for option in aws_acm_certificate.edge.domain_validation_options :
    option.domain_name => option
  }

  zone_id = var.cloudflare_zone_id
  name    = trimsuffix(each.value.resource_record_name, ".")
  type    = each.value.resource_record_type
  content = trimsuffix(each.value.resource_record_value, ".")
  ttl     = 60
  proxied = false
}

resource "aws_acm_certificate_validation" "edge" {
  certificate_arn         = aws_acm_certificate.edge.arn
  validation_record_fqdns = [for record in cloudflare_dns_record.acm_validation : record.name]

  # The provider default is 75 minutes, and the apply job's chained STS session
  # caps at 60 — so the default would let the job be killed mid-wait, leaving a
  # pending certificate and no explanation in the log. Fail cleanly instead.
  timeouts {
    create = "15m"
  }
}

# --- the load balancer --------------------------------------------------------

resource "aws_security_group" "alb" {
  name        = "er-prod-alb"
  description = "The internet-facing prod ALB: 80 and 443 from anywhere, egress to pod IPs."
  vpc_id      = module.network.vpc_id

  tags = {
    Name = "er-prod-alb"
  }
}

resource "aws_vpc_security_group_ingress_rule" "alb_https" {
  security_group_id = aws_security_group.alb.id
  description       = "HTTPS from the internet"
  ip_protocol       = "tcp"
  from_port         = 443
  to_port           = 443
  cidr_ipv4         = "0.0.0.0/0"
}

# Port 80 is open only to be redirected (the listener below answers 301). It is
# not a path into the application: nothing is forwarded from it.
resource "aws_vpc_security_group_ingress_rule" "alb_http" {
  security_group_id = aws_security_group.alb.id
  description       = "HTTP from the internet, redirected to HTTPS by the listener"
  ip_protocol       = "tcp"
  from_port         = 80
  to_port           = 80
  cidr_ipv4         = "0.0.0.0/0"
}

resource "aws_vpc_security_group_egress_rule" "alb_to_targets" {
  security_group_id = aws_security_group.alb.id
  description       = "To er-web pods in the private subnets"
  ip_protocol       = "tcp"
  from_port         = 3000
  to_port           = 3000
  cidr_ipv4         = module.network.vpc_cidr
}

# The other half of that path, and the one that is easy to forget: with
# `target_type = "ip"` the targets are POD addresses, and a pod IP is a
# secondary address on a node ENI — so the traffic is evaluated against the
# EKS-managed node security group. Without this rule the targets health-check as
# unhealthy with no indication of why.
resource "aws_vpc_security_group_ingress_rule" "nodes_from_alb" {
  security_group_id            = module.eks.cluster_security_group_id
  description                  = "er-web traffic from the prod ALB"
  ip_protocol                  = "tcp"
  from_port                    = 3000
  to_port                      = 3000
  referenced_security_group_id = aws_security_group.alb.id
}

resource "aws_lb" "edge" {
  name               = "er-prod-edge"
  load_balancer_type = "application"
  internal           = false
  subnets            = module.network.public_subnet_ids
  security_groups    = [aws_security_group.alb.id]

  # Prod: a destroy has to be deliberate. This also means a change Terraform
  # wants to apply by REPLACEMENT will fail rather than quietly recreate the
  # edge — which is the behaviour wanted, and which the destroy guard would
  # catch first anyway.
  enable_deletion_protection = true

  # Reject requests carrying headers the ALB cannot parse, rather than
  # forwarding them for the application to interpret differently than the ALB
  # did. Request-smuggling hygiene; off by default.
  drop_invalid_header_fields = true

  tags = {
    Name = "er-prod-edge"
  }
}

resource "aws_lb_target_group" "web" {
  name        = "er-prod-web"
  port        = 3000
  protocol    = "HTTP"
  target_type = "ip"
  vpc_id      = module.network.vpc_id

  # `/api/healthz` is in the proxy's PUBLIC_PATHS (frontend/src/proxy.ts), which
  # is what this depends on: any path NOT in that list answers a session-less
  # request with a 307 to /login, and a 307 fails this check — so pointing the
  # health check at `/` would mark every healthy pod unhealthy.
  health_check {
    path                = "/api/healthz"
    matcher             = "200"
    interval            = 15
    timeout             = 5
    healthy_threshold   = 2
    unhealthy_threshold = 3
  }

  # The pods are `Recreate`-strategy and behind a blue/green cutover, so a long
  # drain buys nothing; 30s is enough for in-flight requests to finish.
  deregistration_delay = 30

  tags = {
    Name = "er-prod-web"
  }
}

resource "aws_lb_listener" "https" {
  load_balancer_arn = aws_lb.edge.arn
  port              = 443
  protocol          = "HTTPS"

  # TLS 1.2 floor, with 1.3 available. Not the `-TLS-1-0-` or `-TLS-1-1-`
  # policies, which still negotiate protocols no supported client needs.
  ssl_policy = "ELBSecurityPolicy-TLS13-1-2-2021-06"

  # From the VALIDATION resource, not from the certificate: referencing the
  # certificate directly would let the listener be created while the
  # certificate is still `PENDING_VALIDATION`, which fails at apply with a
  # message about the certificate rather than about the ordering.
  certificate_arn = aws_acm_certificate_validation.edge.certificate_arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.web.arn
  }
}

# `www` redirects to the apex rather than serving it.
#
# `CANONICAL_ORIGIN` is one value for the whole internet by design
# (frontend/src/lib/site/routes.ts argues this), so two hostnames serving the
# same pages would split it. Done at the listener because Cloudflare page rules
# are not available to the token this stack uses — and because a redirect that
# lives beside the listener it applies to is visible to anyone reading the edge.
#
# A higher priority number than any future content rule: this one is about the
# hostname, so it should be evaluated after anything path-specific.
resource "aws_lb_listener_rule" "www_to_apex" {
  listener_arn = aws_lb_listener.https.arn
  priority     = 100

  action {
    type = "redirect"

    redirect {
      host        = var.edge_domain
      path        = "/#{path}"
      query       = "#{query}"
      port        = "443"
      protocol    = "HTTPS"
      status_code = "HTTP_301"
    }
  }

  condition {
    host_header {
      values = ["www.${var.edge_domain}"]
    }
  }
}

resource "aws_lb_listener" "http_redirect" {
  load_balancer_arn = aws_lb.edge.arn
  port              = 80
  protocol          = "HTTP"

  # Everything on 80 is redirected; nothing is forwarded. This is the only
  # reason port 80 is open on the security group at all.
  default_action {
    type = "redirect"

    redirect {
      port        = "443"
      protocol    = "HTTPS"
      status_code = "HTTP_301"
    }
  }
}

# --- WAF ----------------------------------------------------------------------
#
# §4.1: "the boundary moves from network to WAF plus application auth, and the
# non-negotiable rules are rate-based." The frontend exposes unauthenticated
# paths, and three of them accept unauthenticated WRITES with no in-app rate
# limiting anywhere — `/api/auth/login`, `/api/auth/password-reset` (which also
# sends email) and `/api/invites`. Those three get rules.
#
# The thresholds differ on purpose, by what abuse costs:
#   - login            — credential stuffing; generous enough for an office
#                        behind one NAT address to sign in normally.
#   - password-reset   — sends email on an unauthenticated request, so abuse
#                        spends money AND the domain's sending reputation,
#                        which is far more expensive to repair than a blocked
#                        request is to retry.
#   - invites          — sends email too, but only plausibly in bursts by a real
#                        admin onboarding a team.
#
# NO MANAGED RULE GROUPS YET, deliberately. `AWSManagedRulesCommonRuleSet`
# inspects request bodies and this is a JSON API that legitimately carries
# SQL-shaped configuration and arbitrary customer column names — exactly the
# payloads its SQLi and XSS rules match on. Adding it blind would reject real
# requests, and with zero traffic there is nothing to tune the exclusions
# against. It belongs in a follow-up, in COUNT mode first.

locals {
  # One shape for all three rules; only name, path and limit vary.
  waf_rate_rules = {
    login = {
      priority = 10
      path     = "/api/auth/login"
      limit    = 100
    }
    password_reset = {
      priority = 20
      path     = "/api/auth/password-reset"
      limit    = 20
    }
    invites = {
      priority = 30
      path     = "/api/invites"
      limit    = 50
    }
  }
}

resource "aws_wafv2_web_acl" "edge" {
  name = "er-prod-edge"

  # NO `§` AND NO PARENTHESES IN THIS STRING, which is not a style preference.
  # WAFv2 validates `description` against
  #   ^[\w+=:#@/\-,\.][\w+=:#@/\-,\.\s]+[\w+=:#@/\-,\.]$
  # so the section marks used throughout this repository, and the brackets that
  # usually accompany them, are both rejected — `CreateWebACL` fails with a
  # `ValidationException` naming the regex and nothing else. It is a 400 at
  # apply time, invisible to `validate` and to `plan`, and it cost one failed
  # prod apply (#911) that had already created the certificate, the load
  # balancer, both listeners and both DNS records before reaching this resource.
  description = "Rate limits on the three unauthenticated write paths - see infrastructure.md 4.1"

  scope = "REGIONAL"

  # Allow by default. This is a rate-limiting ACL, not an allowlist: blocking by
  # default would mean enumerating every legitimate path, and the application's
  # own auth is the access control.
  default_action {
    allow {}
  }

  dynamic "rule" {
    for_each = local.waf_rate_rules

    content {
      name     = "rate-${replace(rule.key, "_", "-")}"
      priority = rule.value.priority

      action {
        block {}
      }

      statement {
        rate_based_statement {
          limit              = rule.value.limit
          aggregate_key_type = "IP"

          # Stated rather than left to the provider default, because the
          # thresholds above are only meaningful against a window and the
          # comment naming them "per 5 min" should be checkable here. WAF
          # accepts 60, 120, 300 or 600.
          evaluation_window_sec = 300

          # Without the scope-down the limit would count EVERY request from an
          # address, so normal browsing would trip a rule meant for one path.
          scope_down_statement {
            byte_match_statement {
              search_string         = rule.value.path
              positional_constraint = "STARTS_WITH"

              field_to_match {
                uri_path {}
              }

              text_transformation {
                priority = 0
                type     = "LOWERCASE"
              }
            }
          }
        }
      }

      visibility_config {
        cloudwatch_metrics_enabled = true
        metric_name                = "er-prod-rate-${replace(rule.key, "_", "-")}"
        # Sampled requests are what the §3.3 acceptance probe reads to confirm
        # WHICH rule blocked, and what rule tuning is done from. They retain
        # URI and headers for 3 hours — not bodies, so a login sample carries
        # no password.
        sampled_requests_enabled = true
      }
    }
  }

  visibility_config {
    cloudwatch_metrics_enabled = true
    metric_name                = "er-prod-edge"
    sampled_requests_enabled   = true
  }

  tags = {
    Name = "er-prod-edge"
  }
}

resource "aws_wafv2_web_acl_association" "edge" {
  resource_arn = aws_lb.edge.arn
  web_acl_arn  = aws_wafv2_web_acl.edge.arn
}

# --- DNS ----------------------------------------------------------------------
#
# Both records are DNS-only (`proxied = false`), which is the decision at the
# top of this file. A proxied record would put Cloudflare's edge in front of the
# ALB and make the AWS web ACL bypassable by addressing the ALB directly.
#
# The apex as a CNAME is legal here specifically because Cloudflare flattens it
# — it answers the apex query with the resolved A records rather than returning
# a CNAME, which plain DNS forbids at a zone apex. That is a Cloudflare
# property, available on the Free plan, and it is why no ALIAS record or static
# address is needed.

resource "cloudflare_dns_record" "apex" {
  zone_id = var.cloudflare_zone_id
  name    = var.edge_domain
  type    = "CNAME"
  content = aws_lb.edge.dns_name
  proxied = false

  # 60s while the edge is being built, so a mistake is correctable in a minute
  # rather than cached for an hour. Worth raising once this is settled.
  ttl = 60

  comment = "Terraform (envs/prod/edge.tf) — the prod ALB. DNS-only: AWS WAF is the edge."
}

resource "cloudflare_dns_record" "www" {
  zone_id = var.cloudflare_zone_id
  name    = "www.${var.edge_domain}"
  type    = "CNAME"
  content = aws_lb.edge.dns_name
  proxied = false
  ttl     = 60

  comment = "Terraform (envs/prod/edge.tf) — redirected to the apex by the ALB listener rule."
}

output "edge_alb_dns_name" {
  description = "The ALB hostname. Addressable directly, which is how the edge is verified before DNS is trusted."
  value       = aws_lb.edge.dns_name
}

output "edge_certificate_arn" {
  value = aws_acm_certificate_validation.edge.certificate_arn
}

output "edge_target_group_arn" {
  description = "What the chart's TargetGroupBinding joins the er-web Service to (#556)."
  value       = aws_lb_target_group.web.arn
}

output "edge_web_acl_arn" {
  value = aws_wafv2_web_acl.edge.arn
}
