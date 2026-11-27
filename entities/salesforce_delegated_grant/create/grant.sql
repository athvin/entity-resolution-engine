INSERT INTO public.salesforce_delegated_grants
  (id, org_id, salesforce_connection_id, sf_org_id, sf_user_id, sf_username, refresh_token,
   scopes, allow_automatic, authorized_by)
VALUES
  ($id, $org_id, $salesforce_connection_id, $sf_org_id, $sf_user_id, $sf_username, $refresh_token,
   $scopes, $allow_automatic, $authorized_by)
