-- templated: __splink__agreement_pattern_counts
-- physical: __splink__agreement_pattern_counts_f055dd20c
CREATE TABLE __splink__agreement_pattern_counts_f055dd20c AS
select
gamma_dob,
gamma_city,
gamma_email,
count(*) as agreement_pattern_count
from __splink__df_comparison_vectors_8ce534105
group by gamma_dob, gamma_city, gamma_email
