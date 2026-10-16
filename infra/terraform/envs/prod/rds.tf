# The prod control-plane + catalog database: Aurora Serverless v2 (PostgreSQL),
# multi-AZ — a writer plus a reader in a second AZ. Unlike dev (min_capacity 0,
# auto-pause, disposable) prod keeps a warm floor (min 0.5) so real traffic
# never eats a cold start, is deletion-protected, and takes a final snapshot +
# 14-day backups. Right-sized for prod; it costs nothing until applied.
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

  backup_retention_period   = 14
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

resource "aws_rds_cluster_instance" "prod" {
  count = 2 # writer + reader, placed across AZs by the subnet group (multi-AZ HA)

  identifier                   = "er-prod-${count.index + 1}"
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
    ER_PG_HOST     = aws_rds_cluster.prod.endpoint
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
