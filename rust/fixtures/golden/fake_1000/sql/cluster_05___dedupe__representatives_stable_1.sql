-- templated: __dedupe__representatives_stable_1
-- physical: __dedupe__representatives_stable_1_79ccd7308
CREATE TABLE __dedupe__representatives_stable_1_79ccd7308 AS
select *
from __dedupe__representatives_1_225b6c241
where stable = 1
