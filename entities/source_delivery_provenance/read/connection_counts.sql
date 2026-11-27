SELECT source, count(DISTINCT salesforce_connection_id) AS connections
FROM public.source_delivery_provenance
WHERE org_id = $org_id AND dataset_id = $dataset_id
GROUP BY source
ORDER BY source
