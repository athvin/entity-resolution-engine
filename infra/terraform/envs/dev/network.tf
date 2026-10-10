# Task 1.1 (infrastructure.md §4).
module "network" {
  source = "../../modules/network"

  name = "er-dev"
  cidr = "10.0.0.0/16"
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
