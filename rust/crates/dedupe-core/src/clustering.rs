//! Connected components at a threshold, as a SQL algorithm:
//! min-representative propagation with a `stable`
//! flag that retires converged clusters from the working set. `cluster_id` is
//! the minimum node id in each component.

use crate::error::Result;
use crate::session::{Session, TableHandle};
use crate::settings::Settings;
use crate::sql::literals::pyrepr;
use crate::sql::pipeline::CtePipeline;
use crate::stages::uid_expr_for;

/// Threshold expressed on match probability.
pub struct ClusterOptions {
    pub threshold_match_probability: f64,
}

fn materialize_sql(
    session: &Session,
    inputs: &[&TableHandle],
    sql: String,
    name: &str,
) -> Result<TableHandle> {
    let mut p = CtePipeline::new();
    for h in inputs {
        p.add_input(h);
    }
    p.enqueue(sql, name);
    session.materialize(&p)
}

/// Cluster pairwise predictions at a probability threshold. Returns a handle
/// to `(node_id, cluster_id)`.
pub fn cluster_at_threshold(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    predictions: &TableHandle,
    opts: ClusterOptions,
) -> Result<TableHandle> {
    let uid = uid_expr_for(settings);
    let threshold = pyrepr(opts.threshold_match_probability);

    // Nodes: one row per record.
    let nodes = materialize_sql(
        session,
        &[concat],
        format!(
            "select\n    {} as node_id\nfrom {}",
            uid.render_bare(),
            concat.templated
        ),
        "__splink__df_nodes_with_composite_ids",
    )?;

    // Edges from predictions above threshold.
    let edges = materialize_sql(
        session,
        &[predictions],
        format!(
            "select\n    {} as node_id_l,\n    {} as node_id_r,\n    match_probability\nfrom {}\nwhere match_probability >= {threshold}",
            uid.render_suffixed("l"),
            uid.render_suffixed("r"),
            predictions.templated
        ),
        "__splink__df_edges_from_predict",
    )?;

    // Symmetrized neighbours.
    let neighbours = materialize_sql(
        session,
        &[&edges],
        format!(
            "select node_id_l as node_rep, node_id_l as node_id, node_id_r as neighbour_rep, node_id_r as neighbour\nfrom {e}\nwhere match_probability >= {threshold}\nunion all\nselect node_id_r as node_rep, node_id_r as node_id, node_id_l as neighbour_rep, node_id_l as neighbour\nfrom {e}\nwhere match_probability >= {threshold}",
            e = edges.templated
        ),
        "__splink__df_neighbours",
    )?;

    // Initial representatives: each node is its own representative.
    let mut representatives = materialize_sql(
        session,
        &[&nodes],
        format!(
            "select distinct node_id as node_id, node_id as representative, 0 as stable\nfrom {}",
            nodes.templated
        ),
        "__splink__df_representatives",
    )?;

    let mut neighbours_src = neighbours.clone();
    let mut stable_tables: Vec<TableHandle> = Vec::new();
    let mut iteration = 0u32;

    loop {
        iteration += 1;
        // rep_updates: min representative and min stable per old_rep.
        let rep_updates = materialize_sql(
            session,
            &[&neighbours_src, &representatives],
            format!(
                "select old_rep, min(representative) as representative, min(stable) as stable\nfrom (\n  select node_rep as old_rep, neighbour_rep as representative, 0 as stable from {n}\n  union all\n  select representative as old_rep, representative, 1 as stable from {r}\n) as rep_union\ngroup by old_rep",
                n = neighbours_src.templated,
                r = representatives.templated
            ),
            "__splink__rep_updates",
        )?;

        // Push new reps down to nodes. Templated names carry the iteration so
        // the final union references each stable set as a distinct input.
        let new_reps = materialize_sql(
            session,
            &[&rep_updates, &representatives],
            format!(
                "select repr.node_id, r.representative, r.stable\nfrom {ru} as r\nleft join {prev} as repr\non r.old_rep = repr.representative",
                ru = rep_updates.templated,
                prev = representatives.templated
            ),
            &format!("__splink__representatives_{iteration}"),
        )?;

        let stable = materialize_sql(
            session,
            &[&new_reps],
            format!("select * from {} where stable = 1", new_reps.templated),
            &format!("__splink__representatives_stable_{iteration}"),
        )?;
        let unstable = materialize_sql(
            session,
            &[&new_reps],
            format!("select * from {} where stable = 0", new_reps.templated),
            &format!("__splink__representatives_unstable_{iteration}"),
        )?;

        // Re-map neighbour endpoints to new reps; keep only cross-cluster edges.
        let filtered = materialize_sql(
            session,
            &[&neighbours_src, &new_reps],
            format!(
                "select l.representative as node_rep, n.node_id, n.neighbour, r.representative as neighbour_rep\nfrom {n} as n\nleft join {reps} as l on l.node_id = n.node_id\nleft join {reps} as r on n.neighbour = r.node_id\nwhere l.representative <> r.representative",
                n = neighbours_src.templated,
                reps = new_reps.templated
            ),
            &format!("__splink__filtered_neighbours_{iteration}"),
        )?;

        let remaining = session.count(&filtered.physical)?;

        // Clean up the per-iteration intermediates we no longer need.
        session.drop_table(&rep_updates)?;
        session.drop_table(&representatives)?;
        session.drop_table(&new_reps)?;
        session.drop_table(&neighbours_src)?;

        stable_tables.push(stable);

        if remaining == 0 {
            session.drop_table(&filtered)?;
            // Final unstable set joins the retired stable sets.
            representatives = unstable;
            break;
        }
        representatives = unstable;
        neighbours_src = filtered;
    }

    // Final cluster assignment: union of retired stable sets + final unstable.
    let mut union_parts: Vec<String> = stable_tables
        .iter()
        .map(|t| {
            format!(
                "select node_id as node_id, representative as cluster_id\nfrom {}",
                t.templated
            )
        })
        .collect();
    union_parts.push(format!(
        "select node_id as node_id, representative as cluster_id\nfrom {}",
        representatives.templated
    ));

    let mut inputs: Vec<&TableHandle> = stable_tables.iter().collect();
    inputs.push(&representatives);
    let output = materialize_sql(
        session,
        &inputs,
        union_parts.join("\n\nUNION ALL\n\n"),
        "__splink__clustering_output_final",
    )?;

    for t in &stable_tables {
        session.drop_table(t)?;
    }
    session.drop_table(&representatives)?;
    session.drop_table(&neighbours)?;
    session.drop_table(&nodes)?;
    session.drop_table(&edges)?;

    Ok(output)
}

/// Incremental clustering: fold a new batch's edges into an existing cluster
/// assignment without re-running connected components over the whole graph.
///
/// Correctness: the result is *identical* to fully re-clustering the combined
/// edge set, because connected components is a pure function of (nodes, edges)
/// and `cluster_id` is the minimum node id in a component. We exploit this by
/// contracting each existing cluster to a single super-node, running
/// connected components over just the new edges (mapped onto super-nodes),
/// then expanding — far cheaper than re-clustering every node and edge.
///
/// Inputs:
/// - `all_concat`: `__splink__df_concat`-shaped table of ALL records
///   (existing + new).
/// - `existing_clusters`: `(node_id, cluster_id)` for the already-clustered
///   (existing) records.
/// - `new_predictions`: scored pairs involving new records (its `unique_id_l`
///   / `unique_id_r` / `match_probability` columns); pairs below the threshold
///   are ignored.
///
/// Returns `(node_id, cluster_id)` for every record. This is dedupe-scoped
/// (single bare unique-id column).
pub fn incremental_cluster(
    session: &Session,
    settings: &Settings,
    all_concat: &TableHandle,
    existing_clusters: &TableHandle,
    new_predictions: &TableHandle,
    opts: ClusterOptions,
) -> Result<TableHandle> {
    let uid = uid_expr_for(settings);
    let uid_col = &settings.unique_id_column;

    // Every node's seed representative: its existing cluster id, or itself if
    // it is a brand-new record not present in the existing assignment.
    let all_reps = materialize_sql(
        session,
        &[all_concat, existing_clusters],
        format!(
            "select n.node_id, coalesce(e.cluster_id, n.node_id) as seed_rep\nfrom (select {node} as node_id from {concat}) as n\nleft join {existing} as e on n.node_id = e.node_id",
            node = uid.render_bare(),
            concat = all_concat.templated,
            existing = existing_clusters.templated,
        ),
        "__splink__df_incremental_reps",
    )?;

    // Super-node graph: distinct seed reps are the nodes, and each new edge
    // (above threshold) becomes an edge between its endpoints' seed reps.
    let seed_nodes = materialize_sql(
        session,
        &[&all_reps],
        format!(
            "select distinct seed_rep as \"{uid_col}\" from {}",
            all_reps.templated
        ),
        "__splink__df_incremental_seed_nodes",
    )?;
    let threshold = pyrepr(opts.threshold_match_probability);
    let seed_edges = materialize_sql(
        session,
        &[new_predictions, &all_reps],
        format!(
            "select al.seed_rep as \"{uid_col}_l\", ar.seed_rep as \"{uid_col}_r\", p.match_probability\nfrom {preds} as p\njoin {reps} as al on al.node_id = {uid_l}\njoin {reps} as ar on ar.node_id = {uid_r}\nwhere p.match_probability >= {threshold}",
            preds = new_predictions.templated,
            reps = all_reps.templated,
            uid_l = uid.render_suffixed("l"),
            uid_r = uid.render_suffixed("r"),
        ),
        "__splink__df_incremental_seed_edges",
    )?;

    // Connected components over the (small) super-node graph.
    let seed_clusters = cluster_at_threshold(session, settings, &seed_nodes, &seed_edges, opts)?;

    // Expand: map every node through its seed rep to the merged cluster id.
    let output = materialize_sql(
        session,
        &[&all_reps, &seed_clusters],
        format!(
            "select ar.node_id, sc.cluster_id\nfrom {reps} as ar\njoin {seed} as sc on ar.seed_rep = sc.node_id",
            reps = all_reps.templated,
            seed = seed_clusters.templated,
        ),
        "__splink__clustering_output_final",
    )?;

    session.drop_table(&all_reps)?;
    session.drop_table(&seed_nodes)?;
    session.drop_table(&seed_edges)?;
    session.drop_table(&seed_clusters)?;
    Ok(output)
}

/// Node + cluster graph metrics for a clustering (node and cluster tables —
/// edge `is_bridge` lives in [`edge_bridges`]). Returns `(nodes, clusters)`
/// handles.
///
/// `clustered` must carry `cluster_id` plus the id columns (e.g. the output of
/// [`clustered_with_input_data`]). Threshold is applied to the predictions.
pub fn graph_metrics(
    session: &Session,
    settings: &Settings,
    predictions: &TableHandle,
    clustered: &TableHandle,
    threshold_match_probability: f64,
) -> Result<(TableHandle, TableHandle)> {
    let uid = uid_expr_for(settings);
    let threshold = pyrepr(threshold_match_probability);
    let node = uid.render("c");
    let edge_l = uid.render_suffixed("l");
    let edge_r = uid.render_suffixed("r");

    // Node metrics: degree (neighbours at/above threshold) + centrality.
    let mut np = CtePipeline::new()
        .with_input(predictions)
        .with_input(clustered);
    np.enqueue(
        format!(
            "select * from {} where match_probability >= {threshold}",
            predictions.templated
        ),
        "__splink__truncated_edges",
    );
    np.enqueue(
        format!(
            "select {edge_l} as node, {edge_r} as neighbour from __splink__truncated_edges\nunion all\nselect {edge_r} as node, {edge_l} as neighbour from __splink__truncated_edges"
        ),
        "__splink__all_nodes",
    );
    np.enqueue(
        format!(
            "select {node} as composite_unique_id, c.cluster_id as cluster_id,\n  count(*) filter (where n.neighbour is not null) as node_degree,\n  count(*) over (partition by c.cluster_id) as cluster_size\nfrom {clustered} as c\nleft join __splink__all_nodes n on {node} = n.node\ngroup by composite_unique_id, cluster_id",
            clustered = clustered.templated
        ),
        "__splink__graph_metrics_node_degree",
    );
    np.enqueue(
        "select composite_unique_id, cluster_id, node_degree,\n  case when cluster_size > 1 then (1.0 * node_degree) / (cluster_size - 1) else 0 end as node_centrality\nfrom __splink__graph_metrics_node_degree".to_string(),
        "__splink__graph_metrics_nodes",
    );
    let nodes = session.materialize(&np)?;

    // Cluster metrics: size, edge count, density, centralisation.
    let mut cp = CtePipeline::new().with_input(&nodes);
    cp.enqueue(
        format!(
            "select cluster_id, count(*) as n_nodes, sum(node_degree)/2.0 as n_edges,\n  case when count(*) > 2 then 1.0*(count(*) * max(node_degree) - sum(node_degree)) / ((count(*) - 1) * (count(*) - 2)) else null end as cluster_centralisation\nfrom {} group by cluster_id",
            nodes.templated
        ),
        "__splink__counts_per_cluster",
    );
    cp.enqueue(
        "select cluster_id, n_nodes, n_edges,\n  case when n_nodes > 1 then 1.0*(n_edges * 2)/(n_nodes * (n_nodes-1)) else null end as density,\n  cluster_centralisation\nfrom __splink__counts_per_cluster".to_string(),
        "__splink__graph_metrics_clusters",
    );
    let clusters = session.materialize(&cp)?;
    Ok((nodes, clusters))
}

/// Edge metrics with `is_bridge`, computed natively in Rust with petgraph
/// (no igraph/Python dependency). A bridge is an edge
/// whose removal increases the number of connected components. Returns a table
/// `(composite_unique_id_l, composite_unique_id_r, is_bridge)`, one row per
/// thresholded prediction edge, orientation preserved.
pub fn edge_bridges(
    session: &Session,
    settings: &Settings,
    predictions: &TableHandle,
    threshold_match_probability: f64,
) -> Result<TableHandle> {
    use petgraph::algo::bridges;
    use petgraph::graphmap::UnGraphMap;
    use petgraph::visit::EdgeRef;

    let uid = uid_expr_for(settings);
    let threshold = pyrepr(threshold_match_probability);

    // Thresholded edges with a stable edge id and string endpoint keys.
    let truncated = materialize_sql(
        session,
        &[predictions],
        format!(
            "select {l} as composite_unique_id_l, {r} as composite_unique_id_r,\n  cast({l} as varchar) as l_key, cast({r} as varchar) as r_key,\n  row_number() over (order by {l}, {r}) - 1 as eid\nfrom {p}\nwhere match_probability >= {threshold}",
            l = uid.render_suffixed("l"),
            r = uid.render_suffixed("r"),
            p = predictions.templated,
        ),
        "__splink__truncated_edges",
    )?;

    let rows = session.query_strings(&format!(
        "select eid, l_key, r_key from {}",
        truncated.physical
    ))?;

    // Build the undirected simple graph (dropping self-loops), matching
    // igraph's semantics. `UnGraphMap::add_edge` already treats (a,b) and (b,a)
    // as one edge and is idempotent, so it deduplicates parallel edges on its
    // own — no separate seen-set is needed (that would double the heap).
    let mut graph: UnGraphMap<&str, ()> = UnGraphMap::new();
    let unordered_key = |a: &str, b: &str| -> (String, String) {
        if a <= b {
            (a.to_string(), b.to_string())
        } else {
            (b.to_string(), a.to_string())
        }
    };
    for r in &rows {
        let (a, b) = (r[1].as_str(), r[2].as_str());
        if a == b {
            continue; // self-loop: never a bridge
        }
        graph.add_edge(a, b, ());
    }

    // Collect bridge edges as unordered key set.
    let mut bridge_keys = std::collections::HashSet::new();
    for e in bridges(&graph) {
        bridge_keys.insert(unordered_key(e.source(), e.target()));
    }

    // Edge ids whose unordered pair is a bridge.
    let bridge_eids: Vec<String> = rows
        .iter()
        .filter(|r| bridge_keys.contains(&unordered_key(&r[1], &r[2])))
        .map(|r| r[0].clone())
        .collect();

    let is_bridge_expr = if bridge_eids.is_empty() {
        "false".to_string()
    } else {
        format!("eid in ({})", bridge_eids.join(", "))
    };
    let mut p = CtePipeline::new().with_input(&truncated);
    p.enqueue(
        format!(
            "select composite_unique_id_l, composite_unique_id_r, ({is_bridge_expr}) as is_bridge from {}",
            truncated.templated
        ),
        "__splink__graph_metrics_edges",
    );
    let out = session.materialize(&p)?;
    session.drop_table(&truncated)?;
    Ok(out)
}

/// Per-threshold cluster summary stats: one row per threshold with
/// `num_clusters`, `max_cluster_size`, `avg_cluster_size`. Each threshold is
/// clustered independently (connected components is deterministic, so this
/// matches the incremental implementation's output).
pub fn cluster_summary_at_thresholds(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    predictions: &TableHandle,
    thresholds: &[f64],
) -> Result<TableHandle> {
    let mut blocks = Vec::new();
    for &t in thresholds {
        let clusters = cluster_at_threshold(
            session,
            settings,
            concat,
            predictions,
            ClusterOptions {
                threshold_match_probability: t,
            },
        )?;
        let weight = if t <= 0.0 || t >= 1.0 {
            "cast(null as float)".to_string()
        } else {
            format!("cast({} as float)", pyrepr((t / (1.0 - t)).log2()))
        };
        blocks.push(format!(
            "select cast({tp} as float) as threshold_match_probability, {weight} as threshold_match_weight, num_clusters, max_cluster_size, avg_cluster_size from (\n  select count(*) as num_clusters, max(cluster_size) as max_cluster_size, avg(cluster_size) as avg_cluster_size from (\n    select cluster_id, count(*) as cluster_size from {c} group by cluster_id))",
            tp = pyrepr(t),
            c = clusters.physical,
        ));
        // Keep the per-threshold cluster table alive until the union runs.
    }
    let mut p = CtePipeline::new();
    p.enqueue(
        blocks.join("\n\nUNION ALL\n\n"),
        "__splink__multi_threshold_summary",
    );
    session.materialize(&p)
}

/// Constrained one-to-one clustering (`cluster_using_single_best_links`):
/// merge clusters only on mutual best links, never putting two records from the
/// same "duplicate-free" source dataset in one cluster. `ties_method`
/// `"lowest_id"` breaks rank ties by the opposite node id (the default);
/// `"drop"` removes tied edges up front. Returns `(node_id, cluster_id)`.
pub fn cluster_using_single_best_links(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    predictions: &TableHandle,
    duplicate_free_datasets: &[String],
    threshold_match_probability: f64,
    ties_method: &str,
) -> Result<TableHandle> {
    let uid = uid_expr_for(settings);
    let sd = &settings.source_dataset_column;
    let threshold = pyrepr(threshold_match_probability);
    let dup_free: Vec<&str> = duplicate_free_datasets
        .iter()
        .map(std::string::String::as_str)
        .collect();

    // Symmetric neighbours above threshold, carrying source datasets.
    let neighbours = materialize_sql(
        session,
        &[predictions],
        format!(
            "select {l} as node_id, {r} as neighbour, \"{sd}_l\" as source_dataset_l, \"{sd}_r\" as source_dataset_r, match_probability\nfrom {p} where match_probability >= {threshold}\nunion all\nselect {r} as node_id, {l} as neighbour, \"{sd}_r\" as source_dataset_l, \"{sd}_l\" as source_dataset_r, match_probability\nfrom {p} where match_probability >= {threshold}",
            l = uid.render_suffixed("l"),
            r = uid.render_suffixed("r"),
            p = predictions.templated,
        ),
        "__splink__sbl_neighbours",
    )?;

    // "drop" ties: remove edges tied to >1 record in a dup-free dataset.
    let neighbours = if ties_method == "drop" {
        drop_ties(session, &neighbours, &dup_free)?
    } else {
        neighbours
    };

    let mut representatives = materialize_sql(
        session,
        &[concat],
        format!(
            "select {node} as node_id, {node} as representative, \"{sd}\" as source_dataset from {c}",
            node = uid.render_bare(),
            c = concat.templated,
        ),
        "__splink__sbl_representatives",
    )?;

    let contains_expr = dup_free
        .iter()
        .map(|d| format!("max(cast(source_dataset = '{d}' as int)) > 0 as contains_{d}"))
        .collect::<Vec<_>>()
        .join(", ");
    let duplicate_criteria = dup_free
        .iter()
        .map(|d| format!("(l.contains_{d} and r.contains_{d})"))
        .collect::<Vec<_>>()
        .join(" or ");

    let mut iteration = 0u32;
    loop {
        iteration += 1;
        let flags = materialize_sql(
            session,
            &[&representatives],
            format!(
                "select representative, {contains_expr} from {} group by representative",
                representatives.templated
            ),
            &format!("__splink__sbl_flags_{iteration}"),
        )?;
        let reps_with_flags = materialize_sql(
            session,
            &[&representatives, &flags],
            format!(
                "select r.node_id, r.source_dataset, cf.*\nfrom {r} as r\ninner join {f} as cf on r.representative = cf.representative",
                r = representatives.templated,
                f = flags.templated,
            ),
            &format!("__splink__sbl_reps_flags_{iteration}"),
        )?;
        let ranked = materialize_sql(
            session,
            &[&neighbours, &reps_with_flags],
            format!(
                "select n.node_id, n.neighbour,\n  rank() over (partition by l.representative order by match_probability desc, r.node_id) as rank_l,\n  rank() over (partition by r.representative order by match_probability desc, l.node_id) as rank_r\nfrom {nb} as n\ninner join {rf} as l on n.node_id = l.node_id\ninner join {rf} as r on n.neighbour = r.node_id\nwhere l.representative <> r.representative and not ({duplicate_criteria})",
                nb = neighbours.templated,
                rf = reps_with_flags.templated,
            ),
            &format!("__splink__sbl_ranked_{iteration}"),
        )?;
        let best = materialize_sql(
            session,
            &[&ranked],
            format!(
                "select node_id, neighbour from {} where rank_l = 1 and rank_r = 1",
                ranked.templated
            ),
            &format!("__splink__sbl_best_{iteration}"),
        )?;
        let new_reps = materialize_sql(
            session,
            &[&best, &representatives],
            format!(
                "select source.node_id, min(source.representative) as representative\nfrom (\n  select b.node_id, repr.representative as representative\n  from {b} as b left join {prev} as repr on b.neighbour = repr.node_id\n  union all\n  select node_id, representative from {prev}\n) as source\ngroup by source.node_id",
                b = best.templated,
                prev = representatives.templated,
            ),
            &format!("__splink__sbl_newrep_{iteration}"),
        )?;
        let next = materialize_sql(
            session,
            &[&new_reps, &representatives],
            format!(
                "select nr.node_id, nr.representative, repr.source_dataset,\n  nr.representative <> repr.representative as needs_updating\nfrom {nr} as nr inner join {prev} as repr on nr.node_id = repr.node_id",
                nr = new_reps.templated,
                prev = representatives.templated,
            ),
            &format!("__splink__sbl_repr_{iteration}"),
        )?;
        let remaining = session.query_scalar_f64(&format!(
            "select count(*)::double from {} where needs_updating",
            next.physical
        ))? as i64;

        for t in [
            &flags,
            &reps_with_flags,
            &ranked,
            &best,
            &new_reps,
            &representatives,
        ] {
            session.drop_table(t)?;
        }
        representatives = next;
        if remaining == 0 {
            break;
        }
    }

    let out = materialize_sql(
        session,
        &[&representatives],
        format!(
            "select node_id, representative as cluster_id from {}",
            representatives.templated
        ),
        "__splink__clustering_output_final",
    )?;
    session.drop_table(&neighbours)?;
    session.drop_table(&representatives)?;
    Ok(out)
}

fn drop_ties(
    session: &Session,
    neighbours: &TableHandle,
    dup_free: &[&str],
) -> Result<TableHandle> {
    let in_list = dup_free
        .iter()
        .map(|d| format!("'{d}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let ties_l = materialize_sql(
        session,
        &[neighbours],
        format!(
            "select node_id, source_dataset_l, source_dataset_r, match_probability,\n  case when count(distinct neighbour) > 1 and source_dataset_r in ({in_list}) then 1 else 0 end as tie_l\nfrom {n} group by node_id, source_dataset_l, source_dataset_r, match_probability",
            n = neighbours.templated
        ),
        "__splink__sbl_ties_l",
    )?;
    let ties_r = materialize_sql(
        session,
        &[neighbours],
        format!(
            "select neighbour, source_dataset_l, source_dataset_r, match_probability,\n  case when count(distinct node_id) > 1 and source_dataset_l in ({in_list}) then 1 else 0 end as tie_r\nfrom {n} group by neighbour, source_dataset_l, source_dataset_r, match_probability",
            n = neighbours.templated
        ),
        "__splink__sbl_ties_r",
    )?;
    let no_ties = materialize_sql(
        session,
        &[neighbours, &ties_l, &ties_r],
        format!(
            "select n.*\nfrom {n} as n\ninner join {l} as l on n.node_id=l.node_id and n.source_dataset_l=l.source_dataset_l and n.source_dataset_r=l.source_dataset_r and n.match_probability=l.match_probability\ninner join {r} as r on n.neighbour=r.neighbour and n.source_dataset_l=r.source_dataset_l and n.source_dataset_r=r.source_dataset_r and n.match_probability=r.match_probability\nwhere l.tie_l = 0 and r.tie_r = 0",
            n = neighbours.templated,
            l = ties_l.templated,
            r = ties_r.templated,
        ),
        "__splink__sbl_no_ties",
    )?;
    session.drop_table(neighbours)?;
    session.drop_table(&ties_l)?;
    session.drop_table(&ties_r)?;
    Ok(no_ties)
}

/// Join the cluster assignment back to the input records.
pub fn clustered_with_input_data(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    clusters: &TableHandle,
) -> Result<TableHandle> {
    let uid = uid_expr_for(settings);
    let node_expr = uid.render("c");
    let sql = format!(
        "select cc.cluster_id, c.*\nfrom {clusters} as cc\nleft join {concat} as c\non cc.node_id = {node_expr}",
        clusters = clusters.templated,
        concat = concat.templated,
    );
    materialize_sql(
        session,
        &[clusters, concat],
        sql,
        "__splink__df_clustered_with_input_data",
    )
}
