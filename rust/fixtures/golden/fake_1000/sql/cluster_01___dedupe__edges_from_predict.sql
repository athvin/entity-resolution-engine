-- templated: __dedupe__edges_from_predict
-- physical: __dedupe__edges_from_predict_562ba6542
CREATE TABLE __dedupe__edges_from_predict_562ba6542 AS
select
    "unique_id_l" as node_id_l,
    "unique_id_r" as node_id_r
    , match_probability
    from __dedupe__predict_70bfbe071
    where match_probability >= 0.95
