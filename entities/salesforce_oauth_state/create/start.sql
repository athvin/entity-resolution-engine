INSERT INTO public.salesforce_oauth_states
  (id, org_id, salesforce_connection_id, state_sha256, code_verifier, redirect_uri,
   allow_automatic, requested_by, expires_at)
VALUES
  ($id, $org_id, $salesforce_connection_id, $state_sha256, $code_verifier, $redirect_uri,
   $allow_automatic, $requested_by, $expires_at)
