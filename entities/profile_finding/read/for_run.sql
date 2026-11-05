SELECT code, severity, column_name, message, detail
FROM lake.main.profile_findings AT (VERSION => $snapshot)
WHERE profile_run_id = $profile_run_id
ORDER BY code, column_name
