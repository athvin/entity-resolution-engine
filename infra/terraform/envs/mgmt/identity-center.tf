# Task 0.3: the §8.1 permission sets, groups, and assignments. The prod
# assignments materialize when prod_account_id is set (account is quota-pending).
module "identity_center" {
  source = "../../modules/identity-center"

  admin_username     = "dupezone"
  nonprod_account_id = "660360495170"
  prod_account_id    = null
}
