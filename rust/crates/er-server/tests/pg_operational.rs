//! Real-Postgres proof for the operational projections: class aggregation,
//! archived/control exclusion, and an empty queue after work drains.

use crate::common;
use er_server::pg;

#[tokio::test]
async fn snapshots_count_live_pod_claims_and_queued_jobs() {
    let Some(db) = common::fresh_db("operational").await else {
        return;
    };
    let mut client = pg::connect(db.dsn()).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let cases = [
        ("queued", "pod", Some(true)),
        ("running", "pod", Some(true)),
        ("restarting", "pod", Some(true)),
        ("canceling", "pod", Some(true)),
        ("running", "pod", None),
        ("running", "control", Some(true)),
        ("failed", "pod", Some(true)),
    ];
    for (index, (state, kind, live)) in cases.into_iter().enumerate() {
        let name = format!("org-{index}");
        pg::ensure_org(&client, &name, "/tmp/config.yaml")
            .await
            .unwrap();
        let org = pg::org_id_by_name(&client, &name).await.unwrap().unwrap();
        let job = pg::enqueue(
            &client,
            &org,
            None,
            "train",
            0,
            None,
            None,
            pg::Provenance::default(),
        )
        .await
        .unwrap();
        client
            .execute(
                "UPDATE jobs SET resource_class = 'S' WHERE id = $1",
                &[&job.id],
            )
            .await
            .unwrap();
        client.execute(
            "INSERT INTO task_instances (id, org_id, job_id, task_id, seq, state, kind, live, resource_class) VALUES ($1, $2, $3, 'train', 0, $4, $5, $6, 'S')",
            &[&format!("task-{index}"), &org, &job.id, &state, &kind, &live],
        ).await.unwrap();
    }
    let queued = er_entities::job::read::operational_queue(&client)
        .await
        .unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].resource_class.as_deref(), Some("S"));
    assert_eq!(queued[0].depth, Some(7));
    let active = er_entities::task_instance::read::operational_active(&client)
        .await
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].active, Some(4));
    er_server::ops::snapshot(&client).await.unwrap();
    client
        .execute("UPDATE jobs SET state = 'failed'", &[])
        .await
        .unwrap();
    client
        .execute("UPDATE task_instances SET state = 'failed'", &[])
        .await
        .unwrap();
    assert_eq!(
        er_entities::job::read::operational_queue(&client)
            .await
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        er_entities::task_instance::read::operational_active(&client)
            .await
            .unwrap()
            .len(),
        0
    );
    er_server::ops::snapshot(&client).await.unwrap();
}
