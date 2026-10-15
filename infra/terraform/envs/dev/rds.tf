# The dev control-plane + catalog database: Aurora Serverless v2 (PostgreSQL).
# "Pay for what you need" — v2 scales ACUs with load and down to 0 when idle
# (min_capacity 0), so an unused dev DB bills ~nothing, unlike the always-on
# in-cluster er-postgres StatefulSet (which also pins an EBS volume). One shared
# cluster; dev_env.sh creates a database per developer on it. Prod gets its own
# provisioned cluster (separate env), never this one.

# A stable, valid Aurora PostgreSQL version (avoids pinning a string that may
# not exist in-region).
data "aws_rds_engine_version" "aurora_pg" {
  engine       = "aurora-postgresql"
  default_only = true
}

resource "aws_db_subnet_group" "dev" {
  name       = "er-dev"
  subnet_ids = module.network.private_subnet_ids
}

# Postgres reachable only from inside the VPC (the EKS pods); no public access.
resource "aws_security_group" "db" {
  name        = "er-dev-db"
  description = "er-dev Aurora: PostgreSQL from the cluster VPC only"
  vpc_id      = module.network.vpc_id

  ingress {
    description = "PostgreSQL from the VPC (EKS pods)"
    from_port   = 5432
    to_port     = 5432
    protocol    = "tcp"
    cidr_blocks = ["10.0.0.0/16"]
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
  # Keep it DSN/URL-safe (no quoting surprises in `host=… password=…`).
  special = false
}

resource "aws_rds_cluster" "dev" {
  cluster_identifier = "er-dev"
  engine             = "aurora-postgresql"
  engine_mode        = "provisioned" # Serverless v2 runs under the provisioned engine
  engine_version     = data.aws_rds_engine_version.aurora_pg.version

  database_name   = "er"
  master_username = "er"
  master_password = random_password.db.result

  db_subnet_group_name   = aws_db_subnet_group.dev.name
  vpc_security_group_ids  = [aws_security_group.db.id]
  storage_encrypted       = true
  backup_retention_period = 1 # dev: a day is plenty
  skip_final_snapshot     = true
  deletion_protection     = false # dev is disposable (dev-down tears it)
  apply_immediately       = true

  serverlessv2_scaling_configuration {
    min_capacity = 0 # scale to 0 (auto-pause) when idle — the pay-per-use win
    max_capacity = 4
  }
}

resource "aws_rds_cluster_instance" "dev" {
  identifier         = "er-dev-1"
  cluster_identifier = aws_rds_cluster.dev.id
  instance_class     = "db.serverless"
  engine             = aws_rds_cluster.dev.engine
  engine_version     = aws_rds_cluster.dev.engine_version
}

# Connection info for scripts/dev_env.sh + the chart's ERSERVER_DSN. Mirrors the
# er/dev/shared/s3 pattern (one shared entry the per-dev secrets build on).
resource "aws_secretsmanager_secret" "db" {
  name        = "er/dev/shared/postgres"
  description = "The dev Aurora Serverless v2 endpoint + master credentials; consumed by scripts/dev_env.sh to build each developer's ERSERVER_DSN."
}

resource "aws_secretsmanager_secret_version" "db" {
  secret_id = aws_secretsmanager_secret.db.id

  secret_string = jsonencode({
    ER_PG_HOST     = aws_rds_cluster.dev.endpoint
    ER_PG_RO_HOST  = aws_rds_cluster.dev.reader_endpoint
    ER_PG_PORT     = 5432
    ER_PG_USER     = "er"
    ER_PG_PASSWORD = random_password.db.result
    ER_PG_DBNAME   = "er"
    # Convenience DSN for the control-plane (libpq keyword form).
    ERSERVER_DSN = "host=${aws_rds_cluster.dev.endpoint} port=5432 user=er password=${random_password.db.result} dbname=er sslmode=require"
  })
}

output "db_endpoint" {
  value = aws_rds_cluster.dev.endpoint
}

output "db_reader_endpoint" {
  value = aws_rds_cluster.dev.reader_endpoint
}
