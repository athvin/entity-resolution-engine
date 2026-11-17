# The prod control-plane + catalog database: Aurora Serverless v2 (PostgreSQL).
# Unlike dev (min_capacity 0, auto-pause, disposable) prod keeps a warm floor
# (min 0.5) so real traffic never eats a cold start, is deletion-protected, and
# takes a final snapshot + 14-day backups.
#
# SINGLE INSTANCE — there is no reader, and therefore no automatic failover.
# This was a writer plus a reader in a second AZ until 2026-11-16. The reader
# cost ~$43/month (a 0.5 ACU floor at $0.12/ACU-hr) and was removed as an owner
# cost decision, after the cluster was measured averaging 0.51 ACU — i.e. pinned
# to its floor, so the reader was buying failover rather than capacity.
#
# Be clear about what that bought and what it cost. An instance or AZ failure is
# now an outage until someone promotes a replacement, and a maintenance restart
# is visible downtime rather than a failover. Restoring HA is `count = 2` and
# nothing else: no application code reads the reader endpoint.
data "aws_rds_engine_version" "aurora_pg" {
  engine       = "aurora-postgresql"
  default_only = true
}

resource "aws_db_subnet_group" "prod" {
  name       = "er-prod"
  subnet_ids = module.network.private_subnet_ids
}

# PostgreSQL reachable only from inside the prod VPC (the EKS pods); no public access.
resource "aws_security_group" "db" {
  name        = "er-prod-db"
  description = "er-prod Aurora: PostgreSQL from the cluster VPC only"
  vpc_id      = module.network.vpc_id

  ingress {
    description = "PostgreSQL from the VPC (EKS pods)"
    from_port   = 5432
    to_port     = 5432
    protocol    = "tcp"
    cidr_blocks = ["10.1.0.0/16"]
  }

  egress {
    description = "all outbound"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "random_password" "db" {
  length = 32
  # DSN/URL-safe (no quoting surprises in `host=… password=…`).
  special = false
}

resource "aws_rds_cluster" "prod" {
  cluster_identifier = "er-prod"
  engine             = "aurora-postgresql"
  engine_mode        = "provisioned" # Serverless v2 runs under the provisioned engine
  engine_version     = data.aws_rds_engine_version.aurora_pg.version

  database_name   = "er"
  master_username = "er"
  master_password = random_password.db.result

  db_subnet_group_name   = aws_db_subnet_group.prod.name
  vpc_security_group_ids = [aws_security_group.db.id]
  storage_encrypted      = true

  backup_retention_period   = 14 # Owner-confirmed production retention; #553.
  deletion_protection       = true
  skip_final_snapshot       = false
  final_snapshot_identifier = "er-prod-final"
  # Prod: don't apply disruptive changes mid-day; roll them in the window.
  apply_immediately = false

  serverlessv2_scaling_configuration {
    min_capacity = 0.5 # warm floor — no cold start on prod traffic
    max_capacity = 16
  }
}

# Keyed by identifier, NOT by count — because which instance is the writer is
# runtime state, not configuration. Aurora swaps the roles on every failover, so
# an index-keyed resource makes "which instance does a shrink destroy" depend on
# that state. Concretely, when this went from two instances to one, `er-prod-2`
# was the writer and `er-prod-1` the reader: a `count = 2 -> 1` shrink destroys
# the HIGHEST index, which would have deleted the writer and forced a failover
# on a live database instead of removing the idle replica. Keyed by name, the
# plan names the instance it is removing and cannot pick the wrong one.
#
# To restore multi-AZ HA, add the second identifier back to the set. Note what
# that costs to undo in the other direction: this is now a SINGLE instance, so
# any change needing an instance reboot — a static cluster parameter, say — is a
# full outage rather than a failover.
#
# The two `moved` blocks that carried these instances off the old index-keyed
# addresses were applied with #1096 and deleted here (#1100); they were inert
# once the state had moved. Re-deriving them is only necessary if this resource
# is ever re-keyed again.
resource "aws_rds_cluster_instance" "prod" {
  for_each = toset(["er-prod-2"])

  identifier                   = each.key
  cluster_identifier           = aws_rds_cluster.prod.id
  instance_class               = "db.serverless"
  engine                       = aws_rds_cluster.prod.engine
  engine_version               = aws_rds_cluster.prod.engine_version
  performance_insights_enabled = true
}

# Endpoint + master credentials for the prod control-plane. Lives under er/prod/*
# — which PlatformAdmin's §8.2 guard denies mutating, so this is written by
# er-mgmt/ProdAdmin only.
resource "aws_secretsmanager_secret" "db" {
  name        = "er/prod/shared/postgres"
  description = "The prod Aurora endpoint + master credentials; consumed by the prod control-plane."
}

resource "aws_secretsmanager_secret_version" "db" {
  secret_id = aws_secretsmanager_secret.db.id

  secret_string = jsonencode({
    ER_PG_HOST = aws_rds_cluster.prod.endpoint
    # With no reader instance the Aurora reader endpoint resolves to the writer,
    # so this stays correct and keeps working if `count` goes back to 2. No
    # application code reads it today.
    ER_PG_RO_HOST  = aws_rds_cluster.prod.reader_endpoint
    ER_PG_PORT     = 5432
    ER_PG_USER     = "er"
    ER_PG_PASSWORD = random_password.db.result
    ER_PG_DBNAME   = "er"
    ERSERVER_DSN   = "host=${aws_rds_cluster.prod.endpoint} port=5432 user=er password=${random_password.db.result} dbname=er sslmode=require"
  })
}

output "db_endpoint" {
  value = aws_rds_cluster.prod.endpoint
}

output "db_reader_endpoint" {
  value = aws_rds_cluster.prod.reader_endpoint
}
