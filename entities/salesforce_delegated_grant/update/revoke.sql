UPDATE public.salesforce_delegated_grants
SET status = 'revoked', revoked_reason = $reason, revoked_at = now(), updated_at = now()
WHERE org_id = $org_id AND salesforce_connection_id = $salesforce_connection_id
  AND status = 'active'
RETURNING id, org_id, salesforce_connection_id, sf_org_id, sf_user_id, sf_username, refresh_token, scopes, allow_automatic, status, authorized_by, revoked_reason, revoked_at, created_at, updated_at
