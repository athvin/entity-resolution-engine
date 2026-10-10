# §7.4: one filesystem per environment, an access point per namespace (those
# arrive with the namespaces in Phase 2), mounted at ERSERVER_CONFIG_ROOT and
# ERSERVER_DROP_ROOT by the API, dispatcher, and every runner Job (§6.2).
# Elastic throughput; content is org YAML projections and CSV import drops.
# CSV drops are tenant data, so the filesystem gets its own CMK.

variable "name" {
  type = string
}

variable "vpc_id" {
  type = string
}

variable "vpc_cidr" {
  type = string
}

variable "subnet_ids" {
  type = list(string)
}

resource "aws_kms_key" "this" {
  description             = "${var.name} EFS (config roots and CSV import drops)"
  enable_key_rotation     = true
  deletion_window_in_days = 30
}

resource "aws_kms_alias" "this" {
  name          = "alias/${var.name}-efs"
  target_key_id = aws_kms_key.this.key_id
}

resource "aws_efs_file_system" "this" {
  creation_token  = var.name
  encrypted       = true
  kms_key_id      = aws_kms_key.this.arn
  throughput_mode = "elastic"

  tags = { Name = var.name }

  lifecycle {
    prevent_destroy = true
  }
}

# "Backup is a checkbox, and ticking it is the decision" (§7.4): AWS Backup's
# default plan — daily, 35-day retention. The only bytes unique to EFS are CSV
# drops awaiting ingest; the org YAMLs are projections of the control-plane DB.
resource "aws_efs_backup_policy" "this" {
  file_system_id = aws_efs_file_system.this.id

  backup_policy {
    status = "ENABLED"
  }
}

resource "aws_security_group" "efs" {
  name_prefix = "${var.name}-efs-"
  description = "NFS from the VPC to ${var.name}"
  vpc_id      = var.vpc_id

  ingress {
    description = "NFS"
    from_port   = 2049
    to_port     = 2049
    protocol    = "tcp"
    cidr_blocks = [var.vpc_cidr]
  }

  tags = { Name = "${var.name}-efs" }
}

resource "aws_efs_mount_target" "this" {
  count = length(var.subnet_ids)

  file_system_id  = aws_efs_file_system.this.id
  subnet_id       = var.subnet_ids[count.index]
  security_groups = [aws_security_group.efs.id]
}

output "file_system_id" {
  value = aws_efs_file_system.this.id
}

output "security_group_id" {
  value = aws_security_group.efs.id
}
