# Task 0.3: the §8.1 permission sets, groups, and assignments — rev 4
# single-account shape, with the §8.2 guard denies riding PlatformAdmin.
module "identity_center" {
  source = "../../modules/identity-center"

  admin_username = "dupezone"
  account_id     = "797781631727"
}
