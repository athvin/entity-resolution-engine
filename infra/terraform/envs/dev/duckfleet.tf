# Separate product, same AWS account and dev cluster. Bootstrap IAM stays platform-owned.
module "duckfleet_ci" {
  source            = "../../modules/duckfleet-ci"
  cluster_arn       = module.eks.cluster_arn
  oidc_provider_arn = data.aws_iam_openid_connect_provider.github.arn
}
