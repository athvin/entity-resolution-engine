INSERT INTO public.salesforce_connections
  (id, org_id, name, instance_url, auth_kind, client_id, client_secret, api_version)
VALUES
  ($connection_id, $org_id, $name, $instance_url, $auth_kind, $client_id, $client_secret, $api_version)
