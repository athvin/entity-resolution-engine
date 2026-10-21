DELETE FROM lake.main.entity_membership
WHERE record_key NOT IN (SELECT record_key FROM __membership_live)
