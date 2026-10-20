-- templated: __dedupe__agreement_pattern_counts
-- physical: __dedupe__agreement_pattern_counts_f055dd20c
CREATE TABLE __dedupe__agreement_pattern_counts_f055dd20c AS
select
gamma_dob,
gamma_city,
gamma_email,
count(*) as agreement_pattern_count
from __dedupe__comparison_vectors_8ce534105
group by gamma_dob, gamma_city, gamma_email
