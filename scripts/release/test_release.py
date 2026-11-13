#!/usr/bin/env python3
"""Focused offline validation; no application, DMG, network or Git mutations."""
import hashlib
import json
import plistlib
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
import package
import privacy_audit
import verify_artifacts


class ReleaseChecks(unittest.TestCase):
    def test_policy_blocks_publication(self):
        with self.assertRaises(ValueError):
            package.preflight()

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


if __name__ == '__main__':
    unittest.main()
