SELECT count(*) AS scored,
       avg(score) AS mean_score,
       min(score) AS min_score,
       max(score) AS max_score,
       count(*) FILTER (WHERE score < 50) AS below_50,
       count(*) FILTER (WHERE score >= 50 AND score < 80) AS between_50_80,
       count(*) FILTER (WHERE score >= 80) AS at_least_80,
       avg(completeness) AS mean_completeness,
       avg(validity) AS mean_validity,
       avg(freshness) AS mean_freshness
FROM lake.main.record_qualities AT (VERSION => $snapshot)
