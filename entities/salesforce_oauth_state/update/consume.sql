UPDATE public.salesforce_oauth_states
SET consumed_at = now(), updated_at = now()
WHERE state_sha256 = $state_sha256 AND org_id = $org_id
  AND salesforce_connection_id = $salesforce_connection_id
  AND requested_by = $requested_by
  AND consumed_at IS NULL AND expires_at > now()
RETURNING id, org_id, salesforce_connection_id, state_sha256, code_verifier, redirect_uri,
          allow_automatic, requested_by, expires_at, consumed_at, created_at, updated_at
