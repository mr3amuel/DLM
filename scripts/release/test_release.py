#!/usr/bin/env python3
"""Focused offline validation; no application, DMG, network or Git mutations."""
import hashlib
import io
import json
import plistlib
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
import package
import privacy_audit
import verify_artifacts


class ReleaseChecks(unittest.TestCase):
    def test_owner_approved_policy_passes_preflight(self):
        self.assertEqual(package.preflight(), package.metadata())

    def test_each_policy_approval_is_independently_required(self):
        keys = ('publication_approved', 'source_availability_reviewed',
                'third_party_licenses_reviewed', 'history_and_assets_reviewed')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            policy = root / 'scripts/release/policy.json'
            policy.parent.mkdir(parents=True)
            (root / 'SOURCE_AVAILABILITY.md').write_text('Reviewed fixture terms')
            approved = dict.fromkeys(keys, True)
            with patch.object(package, 'ROOT', root):
                for key in keys:
                    for value in (False, None, 'true', 1):
                        with self.subTest(key=key, value=value):
                            policy.write_text(json.dumps({**approved, key: value}))
                            with self.assertRaises(ValueError):
                                package.preflight()
                    incomplete = {k: v for k, v in approved.items() if k != key}
                    policy.write_text(json.dumps(incomplete))
                    with self.assertRaises(ValueError):
                        package.preflight()
                policy.write_text(json.dumps(approved))
                self.assertEqual(package.preflight(), package.metadata())
                (root / 'SOURCE_AVAILABILITY.md').unlink()
                with self.assertRaises(ValueError):
                    package.preflight()

    def test_ring_file_level_attributions_are_required(self):
        inventory = json.loads((package.ROOT / 'third-party/RUST_DEPENDENCIES.json').read_text())
        ring = next(p for p in inventory if p['name'] == 'ring')
        notice = 'third-party/rust/ring-0.17.14/SOURCE-NOTICES.txt'
        self.assertEqual(ring['version'], '0.17.14')
        self.assertIn(notice, ring['notices'])
        text = (package.ROOT / notice).read_text()
        for attribution in ('Copyright 2018 Trent Clarke.', 'Copyright 2016 David Judd.',
                            'Copyright 2016 Simon Sapin.', 'Copyright (c) 2014, Intel Corporation.'):
            self.assertIn(attribution, text)
        self.assertIn('copyright notice and this permission notice appear in all copies.', text)
        self.assertIn('src/debug.rs', text)
        self.assertIn('crypto/fipsmodule/ec/ecp_nistz.c', text)

    def test_metadata_and_untrusted_refs(self):
        version, build = package.metadata()
        self.assertTrue(build.isdigit())
        self.assertEqual(package.metadata('refs/tags/v' + version)[0], version)
        for tag in ['refs/heads/main', 'refs/tags/v0.0.0', 'refs/tags/v1;touch bad', '$(command)']:
            with self.assertRaises(ValueError):
                package.metadata(tag)

    def test_redacted_scan_categories(self):
        text = ('-----BEGIN ' + 'PRIVATE KEY-----').encode()
        self.assertIn('private_key', privacy_audit.findings(text, 'example'))
        self.assertIn('sensitive_filename_candidate', privacy_audit.findings(b'', 'local/history.sqlite3'))
        self.assertNotIn('private_key', privacy_audit.findings(b'PUBLIC KEY', 'chrome-production-public-key.txt'))
        self.assertFalse(privacy_audit.SENSITIVE.search('Cargo.lock'))

    def test_bundle_configuration_resources_arch_and_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            app = Path(directory) / 'DLM.app'
            version, build = package.metadata()
            info = dict(CFBundleIdentifier='com.dlm.native', CFBundleName='DLM',
                        CFBundleShortVersionString=version, CFBundleVersion=build,
                        DLMProduction=True, DLMArchitecture='arm64')
            resources = ['DLMBrand.png', 'DLMIcon.icns', 'Assets.car', 'fa.lproj/Localizable.strings',
                         'DLMChromeExtension/manifest.json', 'DLMFirefoxExtension/manifest.json',
                         'ThirdPartyNotices/RUST_DEPENDENCIES.json', 'ThirdPartyNotices/THIRD_PARTY_NOTICES.md',
                         'ThirdPartyNotices/SOURCE_AVAILABILITY.md']
            for name in resources:
                file = app / 'Contents/Resources' / name
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_bytes(b'fixture')
            for name in ['Contents/MacOS/DLMSwiftExperiment', 'Contents/Helpers/dlm-native-chrome-host']:
                binary = app / name
                binary.parent.mkdir(parents=True, exist_ok=True)
                binary.write_bytes(b'fixture')
            (app / 'Contents/Resources/ThirdPartyNotices/RUST_DEPENDENCIES.json').write_text('[]')
            plist = app / 'Contents/Info.plist'
            plist.write_bytes(plistlib.dumps(info))
            def fake(*args, **kwargs):
                if args[0] == 'lipo': return b'arm64\n'
                if args[0] == 'otool': return b'binary:\n /usr/lib/libSystem.B.dylib (compatibility version 1)\n'
                return b''
            with patch.object(package, 'run', side_effect=fake):
                package.verify_bundle(app, 'arm64')
                notice = app / 'Contents/Resources/ThirdPartyNotices/RUST_DEPENDENCIES.json'
                notice.write_text('[{\"notices\": [\"third-party/rust/missing/LICENSE\"]}]')
                with self.assertRaises(ValueError): package.verify_bundle(app, 'arm64')
                notice.write_text('[]')
                with self.assertRaises(ValueError): package.verify_bundle(app, 'x86_64')
                info['CFBundleIdentifier'] = 'com.dlm.native-development'
                plist.write_bytes(plistlib.dumps(info))
                with self.assertRaises(ValueError): package.verify_bundle(app, 'arm64')
                info['CFBundleIdentifier'] = 'com.dlm.native'
                plist.write_bytes(plistlib.dumps(info))
                (app / 'Contents/Resources/DLMBrand.png').unlink()
                with self.assertRaises(ValueError): package.verify_bundle(app, 'arm64')

    def test_production_path_flags_preserve_existing_flags_and_spaces(self):
        build = (package.NATIVE / 'build.sh').read_text()
        block = build[build.index('# Production panic/source strings'):build.index('architecture=')]
        with tempfile.TemporaryDirectory(prefix='dlm flags ') as directory:
            root = Path(directory)
            env = dict(package.os.environ, configuration='production', repo=str(root / 'source with spaces'),
                       HOME=str(root / 'synthetic home'), CARGO_HOME=str(root / 'registry with spaces'))
            for encoded in ['', '-C' + chr(31) + 'opt-level=2']:
                current = {**env, 'CARGO_ENCODED_RUSTFLAGS': encoded, 'RUSTFLAGS': '-C opt-level=2'}
                result = subprocess.check_output(["/bin/sh", "-ec", block + '\nprintf \'%s\' "$CARGO_ENCODED_RUSTFLAGS"'], env=current).decode()
                flags = result.split(chr(31))
                self.assertEqual(flags[:2], ['-C', 'opt-level=2'])
                self.assertEqual(flags[-1], '--remap-path-prefix=' + current['repo'] + '=/dlm/source')
                self.assertIn('--remap-path-prefix=' + current['HOME'] + '=/dlm/build-user', flags)

    def test_production_binary_privacy_guard(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'fixture'
            for data in [b'/' + b'Users/' + b'synthetic/source.rs', b'/' + b'home/' + b'synthetic/file.rs',
                         b'/' + b'private/tmp/' + b'fixture/build',
                         b'-----BEGIN ' + b'PRIVATE KEY-----', b'gh' + b'p_' + b'X' * 40]:
                binary.write_bytes(data)
                with self.assertRaises(ValueError): package.verify_binary_privacy(binary)
            binary.write_bytes(b'/dlm/source/module.rs /dlm/cargo/crate/src/lib.rs')
            package.verify_binary_privacy(binary)

    def test_exact_artifacts_and_checksums(self):
        version, _ = package.metadata()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for arch in ['arm64', 'x86_64']:
                name = f'DLM-{version}-macOS-{arch}.dmg'
                (root / name).write_bytes(arch.encode())
                digest = hashlib.sha256(arch.encode()).hexdigest()
                (root / (name + '.sha256')).write_text(f'{digest}  {name}\n')
            self.assertEqual(len(verify_artifacts.verify(root)), 4)
            (root / 'private.log').write_bytes(b'never upload')
            with self.assertRaises(ValueError): verify_artifacts.verify(root)
            (root / 'private.log').unlink()
            (root / f'DLM-{version}-macOS-arm64.dmg').write_bytes(b'changed')
            with self.assertRaises(ValueError): verify_artifacts.verify(root)


class InspectionChecks(unittest.TestCase):
    def environment(self, root):
        return {'GITHUB_ACTIONS': 'true', 'RUNNER_ENVIRONMENT': 'github-hosted',
                'GITHUB_EVENT_NAME': 'workflow_dispatch', 'RUNNER_TEMP': str(root)}

    def test_inspection_context_is_explicit_and_outside_checkout(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            for key, bad in [('GITHUB_ACTIONS', 'false'), ('RUNNER_ENVIRONMENT', 'self-hosted'),
                             ('GITHUB_EVENT_NAME', 'push'), ('RUNNER_TEMP', str(package.ROOT)),
                             ('RUNNER_TEMP', 'relative')]:
                with self.subTest(key=key), patch.dict(package.os.environ, {**env, key: bad}, clear=True):
                    with self.assertRaises(ValueError): package._inspection_root()

    def test_normal_package_still_requires_preflight_even_on_inspection_runner(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.dict(package.os.environ, self.environment(Path(directory)), clear=True), \
                 patch.object(package, 'preflight', side_effect=ValueError('blocked')) as gate, \
                 patch.object(package, '_create_verified_dmg') as create:
                with self.assertRaises(ValueError): package.package('arm64')
                gate.assert_called_once_with()
                create.assert_not_called()

    def test_publication_outputs_remain_exclusive_to_gated_entry(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'output'
            name = 'DLM-0.1.0-macOS-arm64.dmg'
            with patch.dict(package.os.environ, {'GITHUB_OUTPUT': str(output)}, clear=True), \
                 patch.object(package, 'preflight', return_value=('0.1.0', '1')) as gate, \
                 patch.object(package, '_create_verified_dmg', return_value=(Path(name), Path(name + '.sha256'))) as create:
                package.package('arm64')
                gate.assert_called_once_with()
                create.assert_called_once_with('arm64', package.ROOT / 'dist', '0.1.0')
            self.assertEqual(output.read_text(), f'dmg=dist/{name}\nchecksum=dist/{name}.sha256\n')

    def exercise(self, failure=None):
        """Mock OS tools only: no build, app, disk image or real mount is executed."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / 'github-output'
            output.write_text('unchanged\n')
            env = {**self.environment(root), 'GITHUB_OUTPUT': str(output)}
            commands = []
            workspace = []
            def fake(*args, **kwargs):
                commands.append(args)
                if args[0] == 'uname':
                    return b'x86_64\n' if failure == 'architecture' else b'arm64\n'
                if args[:2] == ('xcrun', 'swiftc') and failure == 'background':
                    raise subprocess.CalledProcessError(1, args)
                if Path(args[0]).name == 'render-background':
                    if failure == 'background_runtime': raise subprocess.CalledProcessError(1, args)
                    Path(args[-1]).write_bytes(b'synthetic PNG fixture')
                if args[0] == 'dmgbuild':
                    target = Path(args[-1]);workspace.append(target.parent)
                    target.write_bytes(b'synthetic fixture, not a disk image')
                    if failure == 'dmgbuild': raise subprocess.CalledProcessError(1, args)
                if args[:2] == ('hdiutil', 'verify') and failure == 'image_verify':
                    raise subprocess.CalledProcessError(1, args)
                if args[:2] == ('hdiutil', 'attach'):
                    mount = Path(args[args.index('-mountpoint') + 1])
                    if failure == 'attach': raise subprocess.CalledProcessError(1, args)
                    (mount / 'Applications').symlink_to('/Applications')
                    (mount / '.DS_Store').write_bytes(b'fixture')
                    if failure != 'missing_background':
                        (mount / '.background.png').write_bytes(b'wrong' if failure == 'changed_background' else (mount.parent / 'background.png').read_bytes())
                    if failure == 'contents': (mount / 'unexpected.log').write_text('fixture')
                return b''
            def verify(app, architecture):
                if failure == 'source_bundle' or (failure == 'mounted_bundle' and 'mounted' in app.parts):
                    raise ValueError('fixture bundle rejection')
            real_checksum = package.verify_checksum
            def check_checksum(*args):
                if failure == 'checksum': raise ValueError('fixture checksum rejection')
                return real_checksum(*args)
            with patch.dict(package.os.environ, env, clear=True), \
                 patch.object(package, 'run', side_effect=fake), \
                 patch.object(package, 'verify_bundle', side_effect=verify) as bundle, \
                 patch.object(package, 'preflight', side_effect=AssertionError('Inspection called publication gate')), \
                 patch.object(package, 'verify_checksum', side_effect=check_checksum) as checksum:
                if failure:
                    with self.assertRaises((ValueError, subprocess.SubprocessError)):
                        package.inspect_runner_local('arm64')
                else:
                    package.inspect_runner_local('arm64')
                    self.assertEqual(bundle.call_count, 2)
                    checksum.assert_called_once()
                    self.assertTrue(any(c[:2] == ('hdiutil', 'verify') for c in commands))
                if failure in ('attach', 'contents', 'mounted_bundle') or failure is None:
                    self.assertTrue(any(c[:2] == ('hdiutil', 'detach') for c in commands))
                if failure is None:
                    self.assertTrue(any(c[:2] == ('xcrun', 'swiftc') for c in commands))
                    self.assertFalse(any(c[0] == 'swift' for c in commands))
                self.assertEqual(output.read_text(), 'unchanged\n')
                self.assertFalse(list(root.glob('dlm-dmg-inspection-*')))
                self.assertTrue(all(not p.exists() for p in workspace))

    def test_inspection_preserves_checks_and_never_emits_upload_outputs(self):
        self.exercise()

    def test_inspection_cleans_files_on_failure(self):
        for failure in ['architecture', 'source_bundle', 'background', 'dmgbuild',
                        'background_runtime', 'image_verify', 'attach', 'contents', 'missing_background', 'changed_background', 'mounted_bundle', 'checksum']:
            with self.subTest(failure=failure): self.exercise(failure)

    def test_detach_retries_then_retains_a_busy_mount(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / package._INSPECTION_MARKER).write_text(package._INSPECTION_OWNER)
            mount = root / 'images/.packaging-work/mounted'
            mount.mkdir(parents=True)
            error = subprocess.CalledProcessError(1, ['hdiutil'])
            with patch.object(Path, 'is_mount', return_value=True), \
                 patch.object(package, 'run', side_effect=[error, b'']) as run:
                package._detach(mount)
                self.assertEqual(run.call_args_list[1].args, ('hdiutil', 'detach', '-force', str(mount)))
            with patch.object(Path, 'is_mount', return_value=True), \
                 patch.object(package, 'run', side_effect=error), \
                 patch.object(package.shutil, 'rmtree') as remove:
                with self.assertRaises(RuntimeError): package._remove_inspection(root)
                remove.assert_not_called()
                self.assertTrue(root.exists())

    def test_cleanup_only_removes_owned_prefix_and_rejects_symlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory);owned = root / 'dlm-dmg-inspection-fixture';owned.mkdir()
            (owned / package._INSPECTION_MARKER).write_text(package._INSPECTION_OWNER)
            unrelated = root / 'keep';unrelated.mkdir()
            with patch.dict(package.os.environ, self.environment(root), clear=True):
                package.cleanup_inspections()
                self.assertFalse(owned.exists());self.assertTrue(unrelated.exists())
                owned.symlink_to(unrelated, target_is_directory=True)
                with self.assertRaises(ValueError): package.cleanup_inspections()
                self.assertTrue(unrelated.exists())

    def test_cleanup_refuses_unmarked_prefix_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory);unknown = root / 'dlm-dmg-inspection-not-owned';unknown.mkdir()
            (unknown / 'keep').write_text('synthetic data')
            with patch.dict(package.os.environ, self.environment(root), clear=True):
                with self.assertRaises(ValueError): package.cleanup_inspections()
            self.assertTrue((unknown / 'keep').exists())

    def test_verbose_diagnostics_stay_in_inspection_sink(self):
        sink = io.BytesIO()
        error = subprocess.CalledProcessError(1, ['fixture'], output=b'synthetic stdout', stderr=b'synthetic stderr')
        with patch.object(package, '_INSPECTION_LOG', sink), \
             patch.object(package.subprocess, 'check_output', side_effect=error):
            with self.assertRaises(subprocess.CalledProcessError): package.run('fixture')
        self.assertEqual(sink.getvalue(), b'COMMAND: fixture\nsynthetic stdoutsynthetic stderr')
        with patch.object(package, '_INSPECTION_LOG', None), \
             patch.object(package.subprocess, 'check_output', return_value=b'ok') as command:
            package.run('fixture')
            self.assertEqual(command.call_args.kwargs['stderr'], subprocess.DEVNULL)

    def test_local_mode_is_distinct_and_disposable(self):
        with patch.dict(package.os.environ, {}, clear=True), \
             patch.object(package.sys, 'platform', 'darwin'), \
             patch.object(package, '_inspect_temporary') as inspect, \
             patch.object(package, '_inspection_root', side_effect=AssertionError('No GitHub spoofing')):
            package.inspect_local('arm64')
            inspect.assert_called_once_with('arm64', Path(tempfile.gettempdir()).resolve())
            self.assertNotIn('GITHUB_ACTIONS', package.os.environ)
        with patch.dict(package.os.environ, {'GITHUB_ACTIONS': 'true'}, clear=True):
            with self.assertRaises(ValueError): package.inspect_local('arm64')
        with patch.object(package.sys, 'platform', 'win32'):
            with self.assertRaises(ValueError): package.inspect_local('arm64')

    def test_checksum_verifies_exact_final_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            dmg = Path(directory) / 'fixture.dmg';dmg.write_bytes(b'synthetic bytes')
            sidecar = Path(directory) / 'fixture.dmg.sha256'
            sidecar.write_text(f'{hashlib.sha256(dmg.read_bytes()).hexdigest()}  {dmg.name}\n')
            package.verify_checksum(dmg, sidecar)
            dmg.write_bytes(b'changed')
            with self.assertRaises(ValueError): package.verify_checksum(dmg, sidecar)


if __name__ == '__main__':
    unittest.main()
