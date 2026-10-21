CREATE TABLE IF NOT EXISTS public.pair_labels (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  rec_a_key text NOT NULL,
  rec_b_key text NOT NULL,
  verdict text NOT NULL CHECK (verdict IN ('match', 'no_match')),
  origin text NOT NULL DEFAULT 'human' CHECK (origin IN ('human', 'reviewer', 'llm')),
  labeled_by text NOT NULL,
  model_version text,
  score_at_label double precision,
  note text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (org_id, rec_a_key, rec_b_key)
);
CREATE UNIQUE INDEX IF NOT EXISTS pair_labels_org_pair ON public.pair_labels (org_id, rec_a_key, rec_b_key);
CREATE INDEX IF NOT EXISTS pair_labels_org_recent ON public.pair_labels (org_id, created_at);
