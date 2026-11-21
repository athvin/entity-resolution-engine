use serde_json::{Value, json};

use super::{ExecutionError, Failure};

/// Operator-selected capacity. Naming any resources asks DuckFleet for an
/// isolated worker; the service must refuse if its deployment cannot provide it.
#[derive(Clone, serde::Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Compute {
    /// DuckFleet's persistent default: one CPU, 1 GiB memory, 2 GiB disk.
    Persistent,
    Isolated {
        cpu: u16,
        memory_gib: u16,
        disk_gib: u16,
    },
}

/// Identifiers are diagnostic context, never authority to choose a dataset.
/// Constructed by the task executor from its authenticated work order.
#[derive(Clone)]
pub struct TaskOptions {
    pub compute: Compute,
    pub statement_seconds: u64,
    pub job_id: String,
    pub task_id: String,
    pub attempt_id: String,
}

impl TaskOptions {
    /// Refuse a placement smaller than the executor's declared engine budget.
    /// DuckFleet dedicates half its container memory to DuckDB.
    pub fn require_capacity(
        &self,
        threads: u16,
        engine_memory_bytes: u64,
    ) -> Result<(), ExecutionError> {
        self.validate()?;
        let (cpu, memory_gib) = match self.compute {
            Compute::Persistent => (1, 1),
            Compute::Isolated {
                cpu, memory_gib, ..
            } => (cpu, memory_gib),
        };
        if threads == 0
            || engine_memory_bytes == 0
            || cpu < threads
            || u64::from(memory_gib) * 1024 * 1024 * 1024 / 2 < engine_memory_bytes
        {
            return Err(Failure::Capacity.into());
        }
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<(), ExecutionError> {
        if !(1..=600).contains(&self.statement_seconds)
            || [&self.job_id, &self.task_id, &self.attempt_id]
                .iter()
                .any(|id| {
                    id.is_empty()
                        || id.len() > 128
                        || !id
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                })
            || matches!(self.compute, Compute::Isolated { cpu, memory_gib, disk_gib }
                if cpu == 0 || cpu > 64 || memory_gib == 0 || memory_gib > 512 || disk_gib == 0 || disk_gib > 4096)
        {
            return Err(Failure::Parameter.into());
        }
        Ok(())
    }

    pub(super) fn context(&self) -> Value {
        json!({"er": {"job_id": self.job_id, "task_id": self.task_id, "attempt_id": self.attempt_id}})
    }

    pub(super) fn payload(&self) -> Value {
        let compute = match self.compute {
            Compute::Persistent => json!({}),
            Compute::Isolated {
                cpu,
                memory_gib,
                disk_gib,
            } => {
                json!({"cpu": cpu, "memory": format!("{memory_gib}Gi"), "disk": format!("{disk_gib}Gi")})
            }
        };
        json!({"compute": compute, "context": self.context()})
    }

    pub(super) fn matches(&self, placement: &Value) -> bool {
        let (mode, cpu, memory, disk) = match self.compute {
            Compute::Persistent => ("persistent", 1, 1, 2),
            Compute::Isolated {
                cpu,
                memory_gib,
                disk_gib,
            } => ("isolated", cpu, memory_gib, disk_gib),
        };
        placement["compute"].as_str() == Some(mode)
            && placement["cpu"].as_u64() == Some(u64::from(cpu))
            && placement["memory_bytes"].as_u64() == Some(u64::from(memory) * 1024 * 1024 * 1024)
            && placement["disk_bytes"].as_u64() == Some(u64::from(disk) * 1024 * 1024 * 1024)
    }
}
