"""A hardware experiment must compare real coverage, warm caches and safe targets."""
import copy
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('runner_experiment', Path(__file__).with_name('runner-experiment.py'))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class RunnerExperiment(unittest.TestCase):
    def artifact(self, package, binary):
        return dict(reason='compiler-artifact', executable='/target/'+binary,
                    package_id=package, profile={'test': True}, target={'name': 'main', 'kind': ['test']})

    def test_names_are_attributed_to_packages_and_doctests_and_extensions(self):
        log = '''     Running tests/main.rs (target/debug/deps/main-a)
test tests::same_name ... ok
     Running tests/main.rs (target/debug/deps/main-b)
test tests::same_name ... ok
   Doc-tests package_a
test src/lib.rs - example (line 1) ... ignored
Running extension tests from the already-built dedupe-core binary
test extensions::loads ... ok
test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
'''
        names = runner.identities(log, [self.artifact('a','main-a'), self.artifact('b','main-b')],
                                  {'a':'package-a','b':'package-b'})
        self.assertEqual(names, sorted(['package-a/main/test/tests::same_name:ok',
                                      'package-b/main/test/tests::same_name:ok',
                                      'doctest/package_a/src/lib.rs - example (line 1):ignored',
                                      'dedupe-core/main/test/extensions::loads:ok']))
        coloured = log.replace('Running', '\x1b[32mRunning\x1b[0m')
        self.assertEqual(runner.identities(coloured,
                         [self.artifact('a','main-a'), self.artifact('b','main-b')],
                         {'a':'package-a','b':'package-b'}), names)
        with self.assertRaisesRegex(ValueError, 'every reported test'):
            runner.identities(log.replace('3 passed', '4 passed'),
                              [self.artifact('a','main-a'), self.artifact('b','main-b')],
                              {'a':'package-a','b':'package-b'})

    def test_unknown_missing_or_unattributed_results_fail(self):
        for log in ['', 'test foo ... ok', 'Running tests/main.rs (target/unknown)\ntest foo ... ok']:
            with self.subTest(log=log), self.assertRaises((ValueError, KeyError)):
                runner.identities(log, [], {})

    def test_only_owned_target_is_removed(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); target=root/'rust/target'; target.mkdir(parents=True)
            (target/'object').write_text('rebuildable')
            runner.reset_target(target, root)
            self.assertFalse(target.exists())
            outside=root/'workspace'; outside.mkdir(); (outside/'source').write_text('keep')
            target.symlink_to(outside, target_is_directory=True)
            for bad in [root, outside, target, root/'nested/target']:
                with self.subTest(path=bad), self.assertRaises(ValueError):
                    runner.reset_target(bad, root)
            self.assertEqual((outside/'source').read_text(), 'keep')

    def test_cold_or_broken_cache_and_changed_coverage_are_not_a_warm_comparison(self):
        stats=dict(cache_read_errors=0, cache_write_errors=0,
                   cache_hits={'counts':{'C/C++':305}}, cache_misses={'counts':{}})
        valid=[dict(tests=['real:test']), dict(tests=['real:test'],cache={'stats':stats})]
        self.assertEqual(runner.validate_comparison(valid), 305)
        bads=[]
        bad=copy.deepcopy(valid); bad[1]['tests']=[]; bads.append(bad)
        for key in ['cache_read_errors','cache_write_errors']:
            bad=copy.deepcopy(valid); bad[1]['cache']['stats'][key]=1; bads.append(bad)
        bad=copy.deepcopy(valid); bad[1]['cache']['stats']['cache_hits']['counts']={}; bads.append(bad)
        bad=copy.deepcopy(valid); bad[1]['cache']['stats']['cache_misses']['counts']['C/C++']=1; bads.append(bad)
        for bad in bads:
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                runner.validate_comparison(bad)

    def test_nonmain_cannot_prime_the_protected_cache(self):
        with patch.dict('os.environ', {'GITHUB_REF':'refs/heads/feature'}), \
                patch.object(runner.subprocess, 'check_output', side_effect=AssertionError('guard bypassed')):
            with self.assertRaisesRegex(ValueError, 'protected main'):
                runner.main()

    def test_full_experiment_refuses_a_local_or_self_hosted_checkout(self):
        with patch.dict('os.environ', {'GITHUB_REF':'refs/heads/main', 'GITHUB_ACTIONS':'false'}), \
                patch.object(runner.subprocess, 'check_output', side_effect=AssertionError('guard bypassed')):
            with self.assertRaisesRegex(ValueError, 'GitHub-hosted checkout'):
                runner.main()

    def test_extra_cargo_environment_cannot_split_normal_cache_keys(self):
        with tempfile.TemporaryDirectory() as tmp:
            for variable in ['CARGO_TARGET_DIR', 'CARGO_TERM_COLOR']:
                env={'GITHUB_REF':'refs/heads/main', 'GITHUB_ACTIONS':'true',
                     'EXPERIMENT_HOSTED':'github-hosted', 'GITHUB_WORKSPACE':tmp,
                     'RUNNER_TEMP':tmp, variable:'different'}
                with self.subTest(variable=variable), patch.object(runner, 'ROOT', Path(tmp).resolve()), \
                        patch.dict('os.environ', env), \
                        patch.object(runner.subprocess, 'check_output', side_effect=AssertionError('guard bypassed')):
                    with self.assertRaisesRegex(ValueError, 'comparable cache keys'):
                        runner.main()
