#!/usr/bin/env python3
"""Probe a disposable DuckFleet, preserving failures in a machine-readable report.

Never point this at customer data: catalog tests create uniquely named synthetic
lakes and tables. Does not migrate or attach an existing customer's catalog.
"""
import argparse
import datetime
import hashlib
import json
import os
import platform
from pathlib import Path
import re
import tempfile
import tomllib
import urllib.error
import urllib.request
import uuid

import duckdb
import pyarrow as pa
import pyarrow.ipc as ipc
import pyarrow.parquet as pq

ROOT = Path(__file__).resolve().parents[2]
GOLDEN = ROOT / 'rust/fixtures/golden/fake_1000'


class RemoteError(Exception):
    def __init__(self, status, body):
        self.status = status
        self.body = body
        super().__init__(f'HTTP {status}: {body}')


class Remote:
    def __init__(self, url, token):
        self.url = url.rstrip('/')
        self.token = token
        self.session = None

    def request(self, method, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(self.url + path, data=data, method=method,
            headers={'Authorization': 'Bearer ' + self.token, 'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(request, timeout=90) as response:
                return response.read()
        except urllib.error.HTTPError as error:
            raise RemoteError(error.code, error.read().decode()) from error

    def __enter__(self):
        self.session = json.loads(self.request('POST', '/v1/sessions', {}))['session_id']
        return self

    def __exit__(self, *args):
        if self.session:
            self.request('DELETE', '/v1/sessions/' + self.session)

    def query(self, sql, params=None, timeout=30):
        payload = {'request_id': str(uuid.uuid4()), 'sql': sql,
                   'parameters': params or [], 'timeout_seconds': timeout,
                   'context': {'invocation_id': 'er-compatibility-discovery'}}
        raw = self.request('POST', f'/v1/sessions/{self.session}/queries', payload)
        return ipc.open_stream(raw).read_all()


class Local:
    def __init__(self):
        self.conn = duckdb.connect()
        self.conn.execute('SET threads=1')

    def query(self, sql, params=None):
        result = self.conn.sql(sql, params=params or [])
        return pa.table({}) if result is None else result.to_arrow_table()


def equal(left, right):
    if left.schema != right.schema or left.to_pylist() != right.to_pylist():
        details = next(((i, a, b) for i, (a, b) in enumerate(zip(left.to_pylist(), right.to_pylist())) if a != b), None)
        raise AssertionError(f'result mismatch: {left.schema} vs {right.schema}; rows {left.num_rows}/{right.num_rows}; first difference {details}')


def digest(table):
    return hashlib.sha256(json.dumps(table.to_pylist(), sort_keys=True, default=str).encode()).hexdigest()


def scalar(backend, sql, params=None):
    return backend.query(sql, params).column(0)[0].as_py()


def error_text(backend, sql):
    try:
        backend.query(sql)
    except (RemoteError, duckdb.Error) as error:
        return str(error)
    raise AssertionError('statement unexpectedly succeeded: ' + sql)


def scoring(backend):
    """Replay governed golden SQL, starting at comparison vectors, not toy scores."""
    source = GOLDEN / 'stages/blocked_with_cols.parquet'
    table = pq.read_table(source)
    local = duckdb.connect()
    # Derive DuckDB types from Parquet rather than maintaining a second schema.
    columns = local.execute('DESCRIBE SELECT * FROM read_parquet(?)', [str(source)]).fetchall()
    local.close()
    definitions = ', '.join('"' + name + '" ' + kind for name, kind, *_ in columns)
    backend.query('CREATE TEMP TABLE blocked_with_cols (' + definitions + ')')
    rows = table.to_pylist()
    width = len(table.schema)
    for offset in range(0, len(rows), 100):
        batch = rows[offset:offset + 100]
        placeholders = ','.join('(' + ','.join('?' for _ in range(width)) + ')' for _ in batch)
        params = [row[name] for row in batch for name in table.column_names]
        # Date/decimal parameters use the protocol's documented string transport.
        params = [str(v) if isinstance(v, (datetime.date, datetime.datetime)) else v for v in params]
        backend.query('INSERT INTO blocked_with_cols VALUES ' + placeholders, params)
    for stage in ('05', '06', '07'):
        path, = GOLDEN.glob(f'sql/predict_{stage}_*.sql')
        sql = path.read_text()
        logical = re.search(r'^-- templated: (\w+)', sql, re.M)[1]
        physical = re.search(r'^-- physical: (\w+)', sql, re.M)[1]
        sql = sql.replace('CREATE TABLE ' + physical, 'CREATE TEMP TABLE ' + logical, 1)
        backend.query(sql)
    projection = 'unique_id_l, unique_id_r, match_key, gamma_first_name, gamma_surname, gamma_dob, gamma_city, gamma_email, match_weight, match_probability'
    query = f'SELECT {projection} FROM __dedupe__predict ORDER BY unique_id_l, unique_id_r'
    result = backend.query(query)
    equal(result, backend.query(query))
    return result


def lake_semantics(backend, attach):
    backend.query(attach)
    backend.query("CALL compat_lake.set_option('data_inlining_row_limit', 0)")
    backend.query('CREATE TEMP TABLE scratch_input AS SELECT 1::BIGINT id')
    backend.query('BEGIN')
    backend.query('CREATE TABLE compat_lake.main.records AS SELECT * FROM scratch_input')
    backend.query('COMMIT')
    version = scalar(backend, "SELECT max(snapshot_id) FROM ducklake_snapshots('compat_lake')")
    backend.query('BEGIN')
    backend.query('INSERT INTO compat_lake.main.records VALUES (2)')
    backend.query('COMMIT')
    old = scalar(backend, f'SELECT count(*) FROM compat_lake.main.records AT (VERSION => {version})')
    live = scalar(backend, 'SELECT count(*) FROM compat_lake.main.records')
    backend.query('BEGIN')
    backend.query('INSERT INTO compat_lake.main.records VALUES (3)')
    backend.query('ROLLBACK')
    rolled = scalar(backend, 'SELECT count(*) FROM compat_lake.main.records')
    if (old, live, rolled) != (1, 2, 2):
        raise AssertionError(f'snapshot/commit/rollback counts: {(old, live, rolled)}')
    backend.query('DETACH compat_lake')
    backend.query(attach.replace("DUCKLAKE_VERSION '1.0'", "READ_ONLY, DUCKLAKE_VERSION '1.0'"))
    if scalar(backend, 'SELECT count(*) FROM compat_lake.main.records') != 2:
        raise AssertionError('reopen lost committed output')
    try:
        backend.query('INSERT INTO compat_lake.main.records VALUES (4)')
    except (RemoteError, duckdb.Error) as error:
        if 'read-only' not in str(error).lower():
            raise
    else:
        raise AssertionError('read-only attach accepted a write')
    backend.query('DETACH compat_lake')
    return {'snapshot_rows': old, 'live_rows': live, 'rollback_rows': rolled, 'read_only': True}


EXPECTED_PROBES = {
    'baseline_version_matches_repo', 'engine_version_alignment', 'bound_parameters_and_arrow_types',
    'bound_float_roundtrip', 'session_temp_isolation', 'golden_tolerance_parity', 'exact_scoring_parity',
    'baseline_lake_semantics', 'postgres_metadata_schema_and_lake_semantics',
    'native_201_expression_contract', 'long_task_deadline_admission', 'bulk_result_capacity',
}


def decision(report):
    probes = report['probes']
    complete = len(probes) == len(EXPECTED_PROBES) and {p['name'] for p in probes} == EXPECTED_PROBES
    return 'go' if complete and all(p['passed'] for p in probes) and 'fatal' not in report else 'no-go'


def run(args):
    report = {'decision': 'no-go', 'probes': [], 'limitations': [
        'Discovery replays captured model SQL; it does not exercise the Rust task executor or generated adapter.',
        'The Python baseline uses the same released engine version; it does not prove every bundled Rust build feature.',
        'Cross-build floating-point scoring is held to 1e-12; each backend must be byte-identical on rerun.',
        'Interactive results above 100000 rows/16MiB are explicitly refused; durable jobs carry larger results.',
        'Native 2.0.1 JSON extraction returns NULL, / by zero returns infinity, and % by zero is rejected; no compatibility bridge is enabled.',
        'pgwire nested/binary output requires an explicit VARCHAR cast; generated ER reads use scalar Arrow results.',
        'Synthetic disposable data only; no production isolation or performance claim.'],
        'baseline_engine': duckdb.__version__, 'python_arrow': pa.__version__,
        'duckfleet_revision': args.duckfleet_revision,
        'baseline_platform': platform.platform(),
        'fixture_sha256': {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                           for path in [GOLDEN / 'stages/blocked_with_cols.parquet', GOLDEN / 'predictions.parquet', *sorted(GOLDEN.glob('sql/predict_0[567]*.sql'))]}}

    def probe(name, work):
        try:
            value = work()
            report['probes'].append({'name': name, 'passed': True, 'evidence': value})
        except Exception as error:
            # Fixtures contain synthetic values only; do not send customer data here.
            report['probes'].append({'name': name, 'passed': False, 'error': str(error)})
        print(name + ': ' + ('PASS' if report['probes'][-1]['passed'] else 'FAIL'), flush=True)

    local = Local()

    def baseline_version():
        config = tomllib.loads((ROOT / 'rust/.cargo/config.toml').read_text())
        expected = re.search(r'DUCKDB_VERSION="v([^"]+)"', config['env']['CXXFLAGS'])[1]
        if duckdb.__version__ != expected:
            raise AssertionError(f'baseline wheel {duckdb.__version__} differs from ER engine {expected}')
        return expected
    probe('baseline_version_matches_repo', baseline_version)
    try:
        with Remote(args.url, os.environ['DUCKFLEET_TOKEN']) as remote:
            report['remote_engine'] = scalar(remote, 'SELECT version()')
            report['extensions'] = remote.query('SELECT extension_name, extension_version FROM duckdb_extensions() WHERE loaded ORDER BY extension_name').to_pylist()

            def versions():
                if report['remote_engine'].lstrip('v') != duckdb.__version__:
                    raise AssertionError(f"engine versions differ: local {duckdb.__version__}, remote {report['remote_engine']}")
                return report['remote_engine']
            probe('engine_version_alignment', versions)

            def types():
                sql = 'SELECT ?::BIGINT id, ?::VARCHAR person_name, ?::DECIMAL(20,4) amount, ?::DATE date_value, ?::TIMESTAMP instant, ?::BOOLEAN flag, ?::VARCHAR missing'
                params = [9223372036854775806, "O'Brien", '1234567890123456.1234', '2026-11-20', '2026-11-20 12:34:56.123456', True, None]
                a, b = local.query(sql, params), remote.query(sql, params)
                equal(a, b)
                return {'schema': str(a.schema), 'sha256': digest(a)}
            probe('bound_parameters_and_arrow_types', types)

            def float_roundtrip():
                fixture = pq.read_table(GOLDEN / 'stages/blocked_with_cols.parquet')
                values = sorted({value for field in fixture.schema if pa.types.is_floating(field.type)
                                 for value in fixture[field.name].to_pylist() if value is not None})
                if not values:
                    raise AssertionError('fixture contains no floating-point values')
                sql = 'SELECT * FROM (VALUES ' + ','.join('(?::DOUBLE)' for _ in values) + ') t(v) ORDER BY v'
                a, b = local.query(sql, values), remote.query(sql, values)
                equal(a, b)
                return {'distinct_values': len(values), 'sha256': digest(a)}
            probe('bound_float_roundtrip', float_roundtrip)

            def temporary():
                remote.query('CREATE TEMP TABLE session_probe AS SELECT 42::BIGINT probe_value')
                if scalar(remote, 'SELECT probe_value FROM session_probe') != 42:
                    raise AssertionError('temporary state lost')
                with Remote(args.url, os.environ['DUCKFLEET_TOKEN']) as other:
                    try:
                        other.query('SELECT * FROM session_probe')
                    except RemoteError as error:
                        if error.status != 400 or 'does not exist' not in error.body:
                            raise
                    else:
                        raise AssertionError('temporary state leaked to another session')
                return 'temporary state survives requests and is absent in another session'
            probe('session_temp_isolation', temporary)

            score_results = []

            def scores():
                a, b = scoring(local), scoring(remote)
                score_results.extend([a, b])
                repeat_local = Local()
                try:
                    equal(a, scoring(repeat_local))
                finally:
                    repeat_local.conn.close()
                with Remote(args.url, os.environ['DUCKFLEET_TOKEN']) as repeat_remote:
                    equal(b, scoring(repeat_remote))
                # The source goldens carry established numeric tolerances, separate
                # from the exact cross-engine equality checked above.
                golden = pq.read_table(GOLDEN / 'predictions.parquet').to_pylist()
                expected = {(row['unique_id_l'], row['unique_id_r']): row for row in golden}
                if a.num_rows != len(expected) or b.num_rows != len(expected) or a.schema != b.schema:
                    raise AssertionError('prediction cardinality differs from golden')
                for row in a.to_pylist() + b.to_pylist():
                    want = expected[(row['unique_id_l'], row['unique_id_r'])]
                    for key, value in row.items():
                        tolerance = {'match_weight': 1e-8, 'match_probability': 1e-10}.get(key)
                        if tolerance is None:
                            if value != want[key]:
                                raise AssertionError('golden differs: ' + key)
                        elif not abs(value - want[key]) <= tolerance:
                            raise AssertionError('golden numeric tolerance exceeded: ' + key)
                deltas = {key: max(abs(x[key] - y[key]) for x, y in zip(a.to_pylist(), b.to_pylist())) for key in ('match_weight', 'match_probability')}
                return {'rows': a.num_rows, 'baseline_sha256': digest(a), 'remote_sha256': digest(b), 'max_delta': deltas, 'fresh_session_rerun': 'exact', 'golden_tolerances': {'weight': 1e-8, 'probability': 1e-10}}
            probe('golden_tolerance_parity', scores)

            def exact_scores():
                if len(score_results) != 2:
                    raise AssertionError('scoring did not complete')
                left, right = score_results
                if left.schema != right.schema or left.num_rows != right.num_rows:
                    raise AssertionError('score schema or row count differs')
                for a, b in zip(left.to_pylist(), right.to_pylist()):
                    for key, value in a.items():
                        other = b[key]
                        if isinstance(value, float):
                            if abs(value - other) > 1e-12:
                                raise AssertionError(f'floating-point score delta exceeds 1e-12: {key}: {value} vs {other}')
                        elif value != other:
                            raise AssertionError(f'non-numeric score field differs: {key}: {value} vs {other}')
                return {'rows': left.num_rows, 'floating_point_tolerance': 1e-12,
                        'repeat_each_backend': 'byte-identical results'}
            probe('exact_scoring_parity', exact_scores)

            def local_lake():
                with tempfile.TemporaryDirectory(prefix='er-compat-') as directory:
                    local.query('INSTALL ducklake')
                    local.query('LOAD ducklake')
                    attach = f"ATTACH 'ducklake:{directory}/metadata.ducklake' AS compat_lake (DATA_PATH '{directory}/data', DUCKLAKE_VERSION '1.0')"
                    return lake_semantics(local, attach)
            probe('baseline_lake_semantics', local_lake)

            def remote_lake():
                # Fixed disposable Compose-only credentials, never a customer DSN.
                suffix = uuid.uuid4().hex
                attach = f"ATTACH 'ducklake:postgres:host=postgres dbname=lake user=duckfleet_catalog password=local-lake-catalog-password' AS compat_lake (DATA_PATH 's3://duckfleet/compat/{suffix}/', METADATA_SCHEMA 'compat_{suffix}', DUCKLAKE_VERSION '1.0')"
                return lake_semantics(remote, attach)
            probe('postgres_metadata_schema_and_lake_semantics', remote_lake)

            def native_201():
                evidence = {
                    'regex_partial_match': scalar(remote, "SELECT 'abc' ~ 'b'"),
                    'factorial_before_multiplication': scalar(remote, 'SELECT 3! * 2'),
                    'binary_literal_type': scalar(remote, "SELECT typeof(X'ff')"),
                    'preserve_case_setting': scalar(remote, 'SELECT current_setting(\'preserve_identifier_case\')'),
                    'single_arrow_json_result': scalar(remote, 'SELECT x -> x + 1 FROM range(1) t(x)'),
                    'division_by_zero_result': scalar(remote, 'SELECT 1 / 0'),
                    'modulo_by_zero_rejected': error_text(remote, 'SELECT 1 % 0'),
                }
                if evidence['regex_partial_match'] is not True or evidence['factorial_before_multiplication'] != 12:
                    raise AssertionError(evidence)
                if evidence['binary_literal_type'] != 'BLOB' or evidence['preserve_case_setting'] != 'preserve_case':
                    raise AssertionError(evidence)
                if evidence['single_arrow_json_result'] is not None:
                    raise AssertionError(evidence)
                if evidence['division_by_zero_result'] != float('inf') or 'Division by zero' not in evidence['modulo_by_zero_rejected']:
                    raise AssertionError(evidence)
                return evidence
            probe('native_201_expression_contract', native_201)

            def deadline():
                remote.query('SELECT 1', timeout=61)
                return 'accepts a deadline above 60 seconds'
            probe('long_task_deadline_admission', deadline)

            def large_result():
                result = remote.query('SELECT i FROM range(100000) t(i)')
                if result.num_rows != 100000:
                    raise AssertionError('bounded result truncated')
                refusal = error_text(remote, 'SELECT i FROM range(100001) t(i)')
                if 'result exceeds local response limit' not in refusal:
                    raise AssertionError('unbounded result was not refused explicitly: ' + refusal)
                return {'returned_rows': result.num_rows, 'overflow': 'explicit bounded refusal'}
            probe('bulk_result_capacity', large_result)
    except Exception as error:
        report['fatal'] = str(error)
    finally:
        report['baseline_extensions'] = local.query('SELECT extension_name, extension_version FROM duckdb_extensions() WHERE loaded ORDER BY extension_name').to_pylist()
        local.conn.close()
    report['decision'] = decision(report)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, default=str) + '\n')
    print(f"{report['decision']}: {args.output}", flush=True)
    return 0 if report['decision'] == 'go' else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--duckfleet-revision', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--disposable-compose', action='store_true', required=True,
                        help='acknowledge this target is a disposable upstream Compose stack')
    raise SystemExit(run(parser.parse_args()))
