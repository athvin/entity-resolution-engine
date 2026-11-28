CREATE TABLE IF NOT EXISTS public.compute_rates (
  id text NOT NULL,
  resource_class text NOT NULL CHECK (resource_class IN ('S', 'M', 'L')),
  billing_period text NOT NULL,
  usd_per_pod_hour double precision NOT NULL,
  attempt_hours double precision NOT NULL,
  billed_usd double precision NOT NULL,
  instances bigint NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (resource_class, billing_period)
);
CREATE UNIQUE INDEX IF NOT EXISTS compute_rates_class_period ON public.compute_rates (resource_class, billing_period);
