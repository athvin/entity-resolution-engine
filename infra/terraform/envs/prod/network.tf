# Prod network — the same module as dev, a distinct /16 so dev and prod never
# overlap (future VPC peering, and no ambiguity in the DB security-group rule).
module "network" {
  source = "../../modules/network"

  name = "er-prod"
  cidr = "10.1.0.0/16"
}

output "vpc_id" {
  value = module.network.vpc_id
}

output "private_subnet_ids" {
  value = module.network.private_subnet_ids
}

output "public_subnet_ids" {
  value = module.network.public_subnet_ids
}
