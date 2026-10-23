SELECT e.id
FROM lake.main.entities e
LEFT JOIN lake.main.entity_membership m ON m.entity_id = e.id
WHERE e.status = 'active'
GROUP BY e.id
HAVING count(m.record_key) = 0
ORDER BY e.id
