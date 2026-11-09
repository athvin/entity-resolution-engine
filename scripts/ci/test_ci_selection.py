"""Regression tests for selective CI and fail-closed required contexts."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def module(name):
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), HERE / (name + '.py'))
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


selector = module('rust-select')


def metadata():
    deps = {'core': [], 'engine': ['core'], 'er-cli': ['engine'], 'er-server': ['engine'],
            'er-mcp': [], 'test-helper': [], 'consumer': ['test-helper'], 'builder': ['core']}
    return {'workspace_members': list(deps), 'packages': [
        {'id': name, 'name': name, 'manifest_path': str(ROOT / 'rust/crates' / name / 'Cargo.toml'),
         'dependencies': [{'name': dep, 'kind': 'dev' if name == 'consumer' else 'build' if name == 'builder' else None} for dep in dependencies]}
        for name, dependencies in deps.items()]}


class Selection(unittest.TestCase):
    def select(self, *paths, full=False):
        return selector.select(paths, metadata(), full)

    def test_reverse_transitive_dependencies(self):
        result = self.select('rust/crates/core/src/lib.rs')
        self.assertEqual(result['packages'], ['builder', 'core', 'engine', 'er-cli', 'er-server'])
        self.assertTrue(result['lake'] and result['server'])

    def test_dev_dependencies(self):
        self.assertEqual(self.select('rust/crates/test-helper/tests/new.rs')['packages'], ['consumer', 'test-helper'])

    def test_leaf(self):
        result = self.select('rust/crates/er-mcp/src/main.rs')
        self.assertEqual(result['packages'], ['er-mcp'])
        self.assertFalse(result['lake'] or result['server'])

    def test_global_and_unknown_inputs(self):
        for path in ['rust/Cargo.lock', 'rust/crates/er-cli/Cargo.toml', 'rust/crates/deleted/src/lib.rs',
                     'entities/org/entity.yaml', '.github/workflows/rust.yml', 'unknown-input']:
            with self.subTest(path=path):
                self.assertEqual(len(self.select(path)['packages']), len(metadata()['packages']))
        self.assertTrue(self.select(full=True)['full'])

    def test_contract_and_entity_artifacts(self):
        self.assertEqual(self.select('frontend/tests/mock-erserver/server.mjs')['packages'], ['er-server'])
        self.assertEqual(self.select('frontend/src/lib/api/envelopes.json')['packages'], ['er-server'])
        self.assertEqual(self.select('frontend/drizzle/meta/0000_snapshot.json')['packages'], [])

    def test_empty_matrix_still_parses(self):
        self.assertEqual(self.select()['test_matrix']['include'][0]['packages'], [])

    def test_each_selected_package_exactly_once(self):
        result = self.select(full=True)
        self.assertEqual(sorted(p for leg in result['test_matrix']['include'] for p in leg['packages']), result['packages'])


class RequiredGate(unittest.TestCase):
    def gate(self, detect, legs):
        return subprocess.run(['bash', str(HERE / 'gate-selection.sh')],
                              env={**os.environ, 'DETECT_RESULT': detect, 'GATE_LEGS': legs},
                              capture_output=True).returncode

    def test_deliberate_skip(self):
        self.assertEqual(self.gate('success', 'test=false,skipped'), 0)

    def test_real_success(self):
        self.assertEqual(self.gate('success', 'test=true,success\nlake=false,skipped'), 0)

    def test_failed_classification_never_green(self):
        for detect in ['failure', 'cancelled', 'skipped', '']:
            self.assertNotEqual(self.gate(detect, 'test=false,skipped'), 0)

    def test_missing_failed_cancelled_or_unexpected_skip_never_green(self):
        for result in ['skipped', 'failure', 'cancelled', '']:
            self.assertNotEqual(self.gate('success', 'test=true,' + result), 0)
        self.assertNotEqual(self.gate('success', ''), 0)


class GreenBase(unittest.TestCase):
    def test_nonancestor_failed_current_and_wrong_event_rejected(self):
        baseline = module('successful-base')
        def run(i, **kw):
            return dict(id=i, head_sha=str(i)*40, conclusion='success', event='push', head_branch='main', **kw)
        runs = [run(1), run(2), run(3)]
        runs[0]['conclusion'] = 'failure'
        with patch.object(baseline, 'same_policy', return_value=True), patch.object(baseline.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1)):
            self.assertEqual(baseline.find_base(runs, 'head', '3'), '')
        with patch.object(baseline, 'same_policy', return_value=True), patch.object(baseline.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)):
            self.assertEqual(baseline.find_base(runs, 'head', '2'), '3'*40)
        with patch.object(baseline, 'same_policy', return_value=False):
            self.assertEqual(baseline.find_base(runs, 'head', '2'), '')


if __name__ == '__main__':
    unittest.main()
