-- templated: __dedupe__representatives_stable_2
-- physical: __dedupe__representatives_stable_2_9d07fd4b4
CREATE TABLE __dedupe__representatives_stable_2_9d07fd4b4 AS
select *
from __dedupe__representatives_2_5b2f5c8a7
where stable = 1
