"""Selection, coverage and evidence-reuse contracts for the local/CI runner."""
import importlib.util
import fcntl
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('test_runner', ROOT / 'scripts/test-run.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class TestRunner(unittest.TestCase):
    def test_full_javascript_dependencies_outside_base_are_fingerprinted(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder) / 'base'
            scripts = Path(folder) / 'integration/scripts'
            root.mkdir()
            scripts.mkdir(parents=True)
            (scripts/'entry.cjs').write_text("require('./helper.cjs');")
            (scripts/'helper.cjs').write_text('module.exports = {};')
            inputs = runner.js_inputs(root, '../integration/scripts/entry.cjs')
            self.assertIn('../integration/scripts/helper.cjs', inputs)
            before = runner.hash_files(root, inputs)
            (scripts/'helper.cjs').write_text('module.exports = {changed:true};')
            self.assertNotEqual(before, runner.hash_files(root, inputs))

    def test_obsolete_update_bridge_options_are_rejected_before_io(self):
        for option in ('--legacy-bridge', '--legacy-root-source', '--benchmark'):
            result = subprocess.run([sys.executable, str(ROOT/'scripts/test-update-integration.py'),
                                     '--release', '/missing/isolated-release', option],
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 2)
            self.assertIn('unrecognized arguments', result.stderr)
        result = subprocess.run([sys.executable, str(ROOT/'scripts/release/publish.py'),
                                 'candidate-unified', '--repo', 'fixture/repo', '--manifest', '/missing/spec',
                                 '--artifacts', '/missing/assets', '--key', '/missing/key',
                                 '--root-public-key', '/missing/public', '--legacy-bridge'],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 2)
        self.assertIn('unrecognized arguments', result.stderr)

    def setUp(self):
        environment = patch.dict(os.environ)
        environment.start()
        self.addCleanup(environment.stop)
        os.environ.pop('TALOCART_TEST_CATALOG', None)

    def test_declared_binary_prerequisites_apply_to_any_group_and_fingerprint(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            group = {'id': 'custom', 'suite': 'tooling',
                     'command': [sys.executable, '-c', 'pass'],
                     'binaries': [{'env': 'TALOCART_TEST_CUSTOM_BINARY', 'default': 'custom'}]}
            with self.assertRaises(runner.UsageError):
                runner.preflight(root, [group], {})
            binary = root / 'override'
            binary.write_text('#!/bin/sh\nexit 0\n')
            binary.chmod(0o700)
            group['env'] = {'TALOCART_TEST_CUSTOM_BINARY': str(binary)}
            runner.preflight(root, [group], {})
            env = runner.child_environment({}, group)
            self.assertIn(str(binary), runner.inputs(root, group, env))
            before = runner.fingerprint(root, group, env, {})
            binary.write_text('#!/bin/sh\nexit 1\n')
            self.assertNotEqual(before, runner.fingerprint(root, group, env, {}))

    def test_catalog_selection_does_not_leak_to_children(self):
        env = runner.child_environment({'TALOCART_TEST_CATALOG':'/private/full.json'}, {})
        self.assertNotIn('TALOCART_TEST_CATALOG', env)

    def test_targeted_groups_are_opt_in_and_full_keeps_original_coverage_once(self):
        groups = runner.catalog(ROOT)
        full = runner.select(groups, ['full'], [])
        self.assertEqual(len(full), 53)
        self.assertEqual([g["id"] for g in full].count("sales-history-dst"), 1)
        self.assertEqual([g["id"] for g in full].count("indexing"), 1)
        self.assertEqual(runner.select(groups, [], ["sales-history-dst"])[0]["command"],
                         ["node", "scripts/verify-sales-history.cjs"])
        self.assertIn("architecture", [g["id"] for g in full])
        self.assertIn("runtime-recovery", [g["id"] for g in full])
        self.assertIn("smtp-recovery", [g["id"] for g in full])
        self.assertEqual([g["id"] for g in full].count("option-order"), 1)
        self.assertEqual([g["id"] for g in full].count("related-products"), 1)
        self.assertEqual([g["id"] for g in full].count("variant-commerce"), 1)
        self.assertEqual([g["id"] for g in full].count("order-editor"), 1)
        self.assertEqual([g["id"] for g in full].count("storefront-images"), 1)
        self.assertFalse(any(g.get('parent') for g in full))
        self.assertEqual([g['id'] for g in runner.select(groups, ['postgres'], [])], ['postgres', 'sales-history-dst'])
        for key, suite, parent in [('postgres-products', 'postgres', 'postgres'),
                                   ('unit-platform', 'rust', 'unit'), ('clippy-web', 'rust', 'clippy')]:
            selected = runner.select(groups, [suite], [key])
            self.assertEqual([g['id'] for g in selected], [key])
            self.assertEqual(selected[0]['parent'], parent)
            with self.assertRaises(runner.UsageError):
                runner.select(groups, [], [key, parent])

    @staticmethod
    def integration_domains():
        paths, owners = {}, {}
        for entry in sorted((ROOT/'crates/web/tests').glob('identity_*.rs')):
            for relative, name in re.findall(r'#\[path = "([^"\n]+)"\]\nmod (\w+);', entry.read_text()):
                if name in owners:
                    raise AssertionError(f"Duplicate domain {name}")
                paths[name] = entry.parent/relative
                owners[name] = entry.stem
        return paths, owners

    def test_postgres_targets_match_existing_domains_and_stay_serial(self):
        paths, owners = self.integration_domains()
        targeted = [g for g in runner.catalog(ROOT) if g.get('parent') == 'postgres']
        whole = [g for g in targeted if g['command'][g['command'].index('--test') + 2] == '--']
        self.assertEqual([g['id'] for g in whole], ['postgres-package-updates'])
        for group in whole:
            command = group['command']
            self.assertEqual(command[command.index('-p') + 1], 'talocart-package-fixture')
            target = command[command.index('--test') + 1]
            self.assertTrue((ROOT / 'crates/package-fixture/tests' / (target + '.rs')).is_file())
            self.assertIn('--ignored', command)
            self.assertIn('--test-threads=1', command)
            self.assertFalse(group['requires_browser'])
        targeted = [g for g in targeted if g not in whole]
        self.assertEqual({g['command'][g['command'].index('--test') + 2][:-2] for g in targeted}, set(owners))
        self.assertEqual(set(owners.values()), {'identity_catalog', 'identity_commerce', 'identity_platform'})
        for group in targeted:
            command = group['command']
            domain = command[command.index('--test') + 2][:-2]
            self.assertEqual(command[command.index('--test') + 1], owners[domain])
            self.assertIn('--ignored', command)
            self.assertIn('--test-threads=1', command)
            source = paths[domain]
            files = [source] + list(source.with_suffix('').rglob('*.rs'))
            self.assertEqual(group['requires_browser'], any(re.search(
                r'(Command::new\("node"\)|browser-[\w-]+\.cjs)', f.read_text()) for f in files if f.is_file()), domain)
        pure = next(g for g in targeted if g['id'] == 'postgres-imports') if 'imports' in owners else next(g for g in targeted if g['id'] == 'postgres-products')
        browser = next(g for g in targeted if g['id'] == 'postgres-orders')
        self.assertFalse(pure['requires_browser'])
        self.assertTrue(browser['requires_browser'])

    def test_postgres_filters_select_only_their_module_with_real_libtest(self):
        groups = [g for g in runner.catalog(ROOT) if g.get('parent') == 'postgres']
        _, owners = self.integration_domains()
        with tempfile.TemporaryDirectory() as temp:
            binaries = {}
            for target in set(owners.values()):
                source, binary = Path(temp)/f'{target}.rs', Path(temp)/target
                source.write_text('\n'.join(
                    f'mod {d} {{ #[test] #[ignore] fn scenario() {{}} '
                    'mod nested { #[test] #[ignore] fn scenario() {} } }' for d, owner in owners.items() if owner == target))
                subprocess.run(['rustc', '--test', str(source), '-o', str(binary)], check=True, capture_output=True)
                binaries[target] = binary
            # Whole-target fixture suites intentionally have no module filter.
            source, binary = Path(temp)/'updates.rs', Path(temp)/'updates'
            source.write_text('#[test] #[ignore] fn scenario() {} #[test] #[ignore] fn other() {}')
            subprocess.run(['rustc', '--test', str(source), '-o', str(binary)], check=True, capture_output=True)
            binaries['updates'] = binary
            for group in groups:
                command = group['command']; pos = command.index('--test')
                filters = command[pos + 2:command.index('--')]
                domain = filters[0][:-2] if filters else None
                args = [*filters, *command[command.index('--') + 1:]]
                output = subprocess.check_output([str(binaries[command[pos + 1]]), *args, '--list'], text=True)
                names = {line.removesuffix(': test') for line in output.splitlines() if line.endswith(': test')}
                with self.subTest(domain=domain):
                    expected = {f'{domain}::scenario', f'{domain}::nested::scenario'} if domain else {'scenario', 'other'}
                    self.assertEqual(names, expected)

    def test_new_integration_entrypoints_and_shared_fixtures_are_fingerprinted(self):
        group = next(g for g in runner.catalog(ROOT) if g['id'] == 'postgres')
        files = runner.inputs(ROOT, group, {})
        for name in ['identity_catalog.rs', 'identity_commerce.rs', 'identity_platform.rs', 'identity/shared.rs']:
            self.assertIn('crates/web/tests/' + name, files)


    def test_exported_sources_track_inputs_without_using_ancestor_git_or_runtime_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            outer = Path(temporary)
            subprocess.run(['git', 'init', '--quiet', str(outer)], check=True, capture_output=True)
            root = outer/'export'; (root/'crates').mkdir(parents=True)
            shared = root/'crates/shared.rs'; shared.write_text('first')
            (root/'target').mkdir(); (root/'target/generated.rs').write_text('ignored')
            (root/'crates/link.rs').symlink_to(root/'target/generated.rs')
            group = {'id':'fixture', 'suite':'rust', 'command':['cargo', 'test']}
            files = runner.inputs(root, group, {})
            self.assertIn('crates/shared.rs', files)
            self.assertNotIn('target/generated.rs', files)
            self.assertNotIn('crates/link.rs', files)
            before = runner.hash_files(root, files)
            shared.write_text('changed')
            self.assertNotEqual(before, runner.hash_files(root, runner.inputs(root, group, {})))



    def test_pure_postgres_preflight_does_not_launch_browser(self):
        group = {'id':'postgres-imports', 'suite':'postgres', 'command':['cargo','test'], 'requires_browser':False}
        env = {'TALOCART_TEST_DATABASE_URL':'postgres://synthetic@127.0.0.1/db_test'}
        with patch.object(runner.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as run:
            runner.preflight(ROOT, [group], env)
            self.assertNotIn('chromium', run.call_args.args[0][2])
        with patch.object(runner.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as run:
            runner.preflight(ROOT, [{**group, 'requires_browser':True}], env)
            self.assertIn('chromium', run.call_args.args[0][2])
        runtime = next(g for g in runner.catalog(ROOT) if g['id'] == 'runtime-recovery')
        with patch.object(runner.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as run, \
                patch.object(runner.os, 'access', return_value=True):
            runner.preflight(ROOT, [runtime], env)
            self.assertNotIn('chromium', run.call_args.args[0][2])

    def test_smtp_recovery_fingerprint_includes_python_maintenance_tools(self):
        group = next(g for g in runner.catalog(ROOT) if g['id'] == 'smtp-recovery')
        inputs = runner.inputs(ROOT, group, {})
        self.assertTrue({'scripts/release/operations.py', 'scripts/release/pg-container.py'} <= inputs)
        self.assertFalse(group['requires_browser'])

    def test_complete_browser_matrix_keeps_all_legacy_groups(self):
        groups = runner.catalog(ROOT)
        browser = [g for g in groups if g['suite'] == 'browser']
        self.assertEqual(len(browser), 41)  # Includes storefront images and the original browser matrix.
        self.assertIn("option-order", {g["id"] for g in browser})
        self.assertEqual({g['id'] for g in browser if not g['id'].startswith(('demo-', 'media-files-'))}, {
            'maintenance', 'updates', 'rich-text', 'error-bulk', 'error-logs', 'product-list-controls', 'dashboard-navigation', 'installation-languages', 'related-products', 'variant-commerce', 'storefront-images',
            'admin-foundation', 'currency-zones', 'cart', 'payments', 'shipping', 'email',
            'checkout-details', 'orders', 'order-editor', 'settings', 'indexing', 'banners', 'attributes', 'customer-groups',
            'customer-fields', 'customer-admin', 'customer-registration', 'option-order', 'option-deletion', 'option-search'})
        for prefix in ['demo-', 'media-files-']:
            chosen = [g for g in browser if g['id'].startswith(prefix)]
            self.assertEqual({g['env']['TALOCART_DEMO_TEST_CASE'] for g in chosen},
                             {'desktop', 'mobile', 'no-js', 'mobile-no-js', 'cli'})
            for group in chosen:
                self.assertEqual(group['env'].get('TALOCART_DEMO_MEDIA_FILES_ONLY'), '1' if prefix == 'media-files-' else None)
        self.assertEqual(len({g['id'] for g in groups}), len(groups))
        self.assertEqual(len(runner.select(groups, ['full'], [])), len([g for g in groups if not g.get('parent')]))
        self.assertEqual([g['id'] for g in runner.select(groups, [], ['settings'])], ['settings'])
        self.assertEqual([g['id'] for g in runner.select(groups, ['browser'], ['settings'])], ['settings'])
        with self.assertRaises(ValueError):
            runner.select(groups, [], ['setting-typo'])

    def test_browser_environment_cannot_silently_narrow_a_full_group(self):
        env = runner.child_environment({'TALOCART_DEMO_UI_ONLY': '1', 'TALOCART_CURRENCY_TEST_CASE': 'mobile',
                                        'TALOCART_TEST_BINARY': 'target/debug/talocart', 'PATH': '/bin'},
                                       {'env': {'TALOCART_DEMO_TEST_CASE': 'no-js'}})
        self.assertNotIn('TALOCART_DEMO_UI_ONLY', env)
        self.assertNotIn('TALOCART_CURRENCY_TEST_CASE', env)
        self.assertEqual(env['TALOCART_DEMO_TEST_CASE'], 'no-js')
        self.assertEqual(env['TALOCART_TEST_BINARY'], 'target/debug/talocart')

    def test_transitive_helpers_invalidate_only_their_consumers(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder); (root/'scripts').mkdir()
            (root/'scripts/a.cjs').write_text("require('./helper.cjs');")
            (root/'scripts/b.cjs').write_text('console.log(1);')
            (root/'scripts/helper.cjs').write_text('module.exports=1;')
            self.assertEqual(runner.js_inputs(root, 'scripts/a.cjs'), {'scripts/a.cjs', 'scripts/helper.cjs'})
            a = runner.hash_files(root, runner.js_inputs(root, 'scripts/a.cjs'))
            b = runner.hash_files(root, runner.js_inputs(root, 'scripts/b.cjs'))
            (root/'scripts/helper.cjs').write_text('module.exports=2;')
            self.assertNotEqual(a, runner.hash_files(root, runner.js_inputs(root, 'scripts/a.cjs')))
            self.assertEqual(b, runner.hash_files(root, runner.js_inputs(root, 'scripts/b.cjs')))
            (root/'scripts/helper.cjs').unlink()
            self.assertNotEqual(a, runner.hash_files(root, runner.js_inputs(root, 'scripts/a.cjs')))

    def test_resume_requires_green_unchanged_recent_evidence_and_log(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder); (root/'proof.log').write_text('passed')
            entry = {'status': 'passed', 'fingerprint': 'a', 'finished': 100, 'log': 'proof.log'}
            self.assertTrue(runner.reusable(entry, 'a', root, now=101))
            for changes in [{'status':'failed'}, {'status':'running'}, {'fingerprint':'b'}, {'finished':-90000}, {'log':'absent.log'}]:
                self.assertFalse(runner.reusable(dict(entry, **changes), 'a', root, now=101))

    def test_real_child_failure_resume_and_mutated_inputs(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); out=root/'logs'; out.mkdir()
            script=root/'case.py'; marker=root/'marker'
            script.write_text("from pathlib import Path\np=Path('marker');p.write_text(p.read_text()+'x' if p.exists() else 'x')\n")
            group={'id':'fixture','suite':'tooling','command':[sys.executable,'case.py']}
            digest=lambda:runner.hash_files(root, {'case.py'})
            state={}; env=os.environ.copy()
            self.assertEqual(runner.execute(group, root, out, env, state, digest, False, 10),0)
            self.assertEqual(runner.execute(group, root, out, env, state, digest, True, 10),0)
            self.assertEqual(marker.read_text(),'x')
            script.write_text("raise SystemExit(7)\n")
            self.assertEqual(runner.execute(group, root, out, env, state, digest, True, 10),7)
            self.assertEqual(state['fixture']['status'],'failed')
            script.write_text("from pathlib import Path\nPath('case.py').write_text('changed')\n")
            self.assertNotEqual(runner.execute(group, root, out, env, state, digest, True, 10),0)
            self.assertEqual(state['fixture']['status'],'stale')

    def test_timeout_is_not_cached_as_success(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); out=root/'logs';out.mkdir();state={}
            group={'id':'slow','suite':'tooling','command':[sys.executable,'-c','import time; time.sleep(5)']}
            self.assertEqual(runner.execute(group,root,out,os.environ.copy(),state,lambda:'same',False,.05),124)
            self.assertEqual(state['slow']['status'],'timeout')

    def test_fingerprint_tracks_sources_binary_runtime_and_private_environment(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder)
            subprocess.run(['git','init','--quiet',str(root)],check=True,capture_output=True)
            (root/'scripts').mkdir(); (root/'apps').mkdir(); (root/'target').mkdir()
            (root/'scripts/case.cjs').write_text('console.log(1);')
            (root/'apps/main.rs').write_text('fn main() {}')
            (root/'target/app').write_bytes(b'binary')
            group={'id':'case','suite':'browser','command':['node','scripts/case.cjs']}
            env={'TALOCART_TEST_BINARY':'target/app','TALOCART_TEST_DATABASE_URL':'private-secret'}
            fingerprint=lambda:runner.fingerprint(root,group,env,{'node':'test-runtime'})
            original=fingerprint()
            self.assertNotIn('private-secret',original)
            (root/'README.md').write_text('Documentation only')
            self.assertEqual(original,fingerprint())
            (root/'apps/main.rs').write_text('fn main() { println!("change"); }')
            changed=fingerprint();self.assertNotEqual(original,changed)
            (root/'target/app').write_bytes(b'new binary')
            binary=fingerprint();self.assertNotEqual(changed,binary)
            env['TALOCART_TEST_DATABASE_URL']='other-private-secret'
            self.assertNotEqual(binary,fingerprint())
            self.assertNotEqual(fingerprint(),runner.fingerprint(root,group,env,{'node':'new-runtime'}))

    def test_runtime_recovery_resume_tracks_the_selected_host_binary(self):
        group = next(g for g in runner.catalog(ROOT) if g['id'] == 'runtime-recovery')
        cases = [({}, 'target/debug/talocart'),
                 ({'TALOCART_TEST_BINARY': 'target/release/talocart'}, 'target/release/talocart'),
                 ({'TALOCART_TEST_BINARY': 'target/release/talocart',
                   'TALOCART_BASE_TEST_BINARY': 'target/base'}, 'target/base'),
                 ({'TALOCART_TEST_BINARY': 'target/release/talocart',
                   'TALOCART_BASE_TEST_BINARY': ''}, 'target/release/talocart')]
        for source, selected in cases:
            with self.subTest(source=source), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                subprocess.run(['git', 'init', '--quiet', str(root)], check=True, capture_output=True)
                (root / '.gitignore').write_text('target/\n')
                binary = root / selected
                binary.parent.mkdir(parents=True)
                binary.write_bytes(b'host-before')
                (root / 'result.log').write_text('passed')
                env = runner.child_environment(source, group)
                digest = lambda: runner.fingerprint(root, group, env, {'node': 'fixture'})
                before = digest()
                entry = {'status': 'passed', 'fingerprint': before, 'finished': 100, 'log': 'result.log'}
                self.assertTrue(runner.reusable(entry, before, root, now=101))
                binary.write_bytes(b'host-after')
                self.assertNotEqual(before, digest())
                self.assertFalse(runner.reusable(entry, digest(), root, now=101))

    def test_runtime_recovery_preflight_checks_override_and_base_precedence(self):
        group = next(g for g in runner.catalog(ROOT) if g['id'] == 'runtime-recovery')
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            release = root / 'release'
            release.write_text('#!/bin/sh\nexit 0\n')
            release.chmod(0o700)
            env = {'TALOCART_TEST_DATABASE_URL': 'postgres://synthetic@127.0.0.1/db_test',
                   'TALOCART_TEST_BINARY': str(release)}
            with patch.object(runner.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as probe:
                runner.preflight(root, [group], env)
                probe.assert_called_once()
                release.unlink()
                probe.reset_mock()
                with self.assertRaisesRegex(runner.UsageError, 'Missing test binary'):
                    runner.preflight(root, [group], env)
                probe.assert_not_called()
                base = root / 'base'
                base.write_text('#!/bin/sh\nexit 0\n')
                base.chmod(0o700)
                runner.preflight(root, [group], {**env, 'TALOCART_BASE_TEST_BINARY': str(base)})
                probe.assert_called_once()

    def test_runtime_recovery_fingerprint_includes_previous_executable_bytes(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            subprocess.run(['git', 'init', '--quiet', str(root)], check=True, capture_output=True)
            (root / 'scripts').mkdir()
            (root / 'target').mkdir()
            (root / 'scripts/test-runtime-recovery.cjs').write_text('console.log(1);')
            (root / 'target/base').write_bytes(b'current')
            (root / 'target/previous').write_bytes(b'previous')
            group = {'id': 'runtime-recovery', 'suite': 'packages',
                     'command': ['node', 'scripts/test-runtime-recovery.cjs']}
            env = {'TALOCART_BASE_TEST_BINARY': 'target/base',
                   'TALOCART_TEST_PREVIOUS_BINARY': 'target/previous'}
            fingerprint = lambda: runner.fingerprint(root, group, env, {'node': 'fixture'})
            before = fingerprint()
            (root / 'target/previous').write_bytes(b'changed previous executable')
            self.assertNotEqual(before, fingerprint())

    def test_new_browser_binary_overrides_are_consistent(self):
        for script in ['browser-settings.cjs','browser-checkout-details.cjs']:
            self.assertIn("process.env.TALOCART_TEST_BINARY", (ROOT/'scripts'/script).read_text())

    def test_second_runner_is_rejected_before_starting_tests(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder); (root/'scripts').mkdir(); output=root/'.local/test-runs';output.mkdir(parents=True)
            (root/'scripts/test-run.py').write_text((ROOT/'scripts/test-run.py').read_text())
            (root/'scripts/test-suite.json').write_text(json.dumps({'version':1,'groups':[
                {'id':'fixture','suite':'tooling','command':[sys.executable,'-c','print("must not run")']}]}))
            with (output/'runner.lock').open('w') as lock:
                fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
                result=subprocess.run([sys.executable,str(root/'scripts/test-run.py'),'--only','fixture'],capture_output=True,text=True)
            self.assertNotEqual(result.returncode,0)
            self.assertIn('Another test runner',result.stderr)
            self.assertNotIn('must not run',result.stdout)

    def test_preflight_rejects_missing_or_application_database_before_execution(self):
        group={'id':'postgres','suite':'postgres','command':[sys.executable,'-c','pass']}
        for value in ['', 'postgres://private@127.0.0.1/shop', 'postgres://private@external/shop_test']:
            with self.assertRaises(runner.UsageError) as error:
                runner.preflight(ROOT,[group],{'TALOCART_TEST_DATABASE_URL':value})
            self.assertNotIn('private',str(error.exception))

    def test_documentation_only_change_does_not_invalidate_rust_evidence(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);(root/'apps').mkdir()
            subprocess.run(['git','init','--quiet',str(root)],check=True,capture_output=True)
            (root/'apps/main.rs').write_text('fn main() {}')
            group={'id':'unit','suite':'rust','command':['cargo','test']}
            before=runner.fingerprint(root,group,{}, {})
            (root/'README.md').write_text('Documentation only')
            self.assertEqual(before,runner.fingerprint(root,group,{}, {}))


if __name__ == '__main__':
    unittest.main()
