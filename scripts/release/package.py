#!/usr/bin/env python3
"""Opt-in headless packaging. No installation, registration or data migration."""
import hashlib
import json
import os
import plistlib
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
NATIVE = ROOT / 'prototypes/swiftui'


def run(*args, **kwargs):
    return subprocess.check_output(args, stderr=subprocess.DEVNULL, **kwargs)


def metadata(ref=''):
    version = (NATIVE / 'release-version.txt').read_text().strip()
    build = (NATIVE / 'release-build.txt').read_text().strip()
    if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', version) or not re.fullmatch(r'[1-9][0-9]*', build):
        raise ValueError('Invalid release metadata')
    if ref and ref != f'refs/tags/v{version}':
        raise ValueError('Tag must match committed release version exactly')
    return version, build


def preflight(ref=''):
    policy = json.loads((ROOT / 'scripts/release/policy.json').read_text())
    if not all(policy.get(k) is True for k in ('publication_approved', 'source_availability_reviewed', 'third_party_licenses_reviewed', 'history_and_assets_reviewed')):
        raise ValueError('Publication blocked: privacy/history/rights approval required')
    if not (ROOT / 'SOURCE_AVAILABILITY.md').is_file():
        raise ValueError('Reviewed source-availability terms are required')
    return metadata(ref)


def verify_bundle(app, architecture):
    version, build = metadata()
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    expected = {'CFBundleIdentifier': 'com.dlm.native', 'CFBundleName': 'DLM',
                'CFBundleShortVersionString': version, 'CFBundleVersion': build,
                'DLMArchitecture': architecture, 'DLMProduction': True}
    if any(info.get(k) != v for k, v in expected.items()):
        raise ValueError('Bundle identity/version/architecture mismatch')
    for relative in ['Contents/Resources/DLMBrand.png', 'Contents/Resources/DLMIcon.icns',
                     'Contents/Resources/Assets.car', 'Contents/Resources/fa.lproj/Localizable.strings',
                     'Contents/Resources/DLMChromeExtension/manifest.json',
                     'Contents/Resources/DLMFirefoxExtension/manifest.json',
                     'Contents/Resources/ThirdPartyNotices/RUST_DEPENDENCIES.json',
                     'Contents/Resources/ThirdPartyNotices/THIRD_PARTY_NOTICES.md',
                     'Contents/Resources/ThirdPartyNotices/SOURCE_AVAILABILITY.md']:
        if not (app / relative).is_file():
            raise ValueError('Required bundle resource missing')
    notices = app / 'Contents/Resources/ThirdPartyNotices'
    for dependency in json.loads((notices / 'RUST_DEPENDENCIES.json').read_text()):
        for path in dependency['notices']:
            relative = Path(path)
            if relative.is_absolute() or '..' in relative.parts or relative.parts[0] != 'third-party':
                raise ValueError('Invalid dependency notice path')
            if not (notices / Path(*relative.parts[1:])).is_file():
                raise ValueError('Dependency redistribution notice missing')
    for relative in ['Contents/MacOS/DLMSwiftExperiment', 'Contents/Helpers/dlm-native-chrome-host']:
        binary = app / relative
        if not binary.is_file():
            raise ValueError('Required executable missing')
        if run('lipo', '-archs', str(binary)).decode().split() != [architecture]:
            raise ValueError('Binary architecture mismatch')
        for line in run('otool', '-L', str(binary)).decode().splitlines()[1:]:
            dependency = line.strip().split(' (')[0]
            if not (dependency.startswith(('/System/Library/', '/usr/lib/', '@rpath/libswift'))):
                raise ValueError('Unexpected non-system dynamic dependency')
    run('codesign', '--verify', '--deep', '--strict', str(app))


def package(architecture):
    version, _ = preflight()
    if architecture not in ('arm64', 'x86_64') or run('uname', '-m').decode().strip() != architecture:
        raise ValueError('Use a matching native macOS runner')
    app = NATIVE / 'build/DLM.app'
    verify_bundle(app, architecture)
    dist = ROOT / 'dist'
    dist.mkdir(exist_ok=True)
    if any(dist.iterdir()):
        raise ValueError('Release staging must start empty')
    name = f'DLM-{version}-macOS-{architecture}.dmg'
    dmg = dist / name
    if dmg.exists():
        raise ValueError('Refusing to overwrite an existing release artifact')
    with tempfile.TemporaryDirectory(prefix='dlm-package-') as directory:
        background = Path(directory) / 'background.png'
        run('swift', str(ROOT / 'scripts/release/background.swift'),
            str(app / 'Contents/Resources/DLMBrand.png'), str(background))
        env = dict(os.environ, DLM_PACKAGE_APP=str(app), DLM_PACKAGE_BACKGROUND=str(background))
        run('dmgbuild', '-s', str(ROOT / 'scripts/release/dmg_settings.py'), f'DLM {version}', str(dmg), env=env)
        run('hdiutil', 'verify', str(dmg))
        mount = Path(directory) / 'mounted'
        mount.mkdir()
        run('hdiutil', 'attach', '-readonly', '-nobrowse', '-mountpoint', str(mount), str(dmg))
        try:
            if not (mount / 'Applications').is_symlink() or os.readlink(mount / 'Applications') != '/Applications':
                raise ValueError('DMG Applications shortcut missing')
            allowed = {'DLM.app', 'Applications', '.DS_Store', '.background', '.VolumeIcon.icns', '.fseventsd', '.Trashes'}
            if not {p.name for p in mount.iterdir()} <= allowed:
                raise ValueError('Unexpected DMG content')
            if not (mount / '.DS_Store').is_file():
                raise ValueError('DMG Finder presentation missing')
            verify_bundle(mount / 'DLM.app', architecture)
        finally:
            run('hdiutil', 'detach', str(mount))
    digest = hashlib.sha256(dmg.read_bytes()).hexdigest()
    (dist / (name + '.sha256')).write_text(f'{digest}  {name}\n')
    if os.environ.get('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
            output.write(f'dmg=dist/{name}\nchecksum=dist/{name}.sha256\n')
    print(name)  # Only basename: never disclose local build paths or logs.


if __name__ == '__main__':
    try:
        if sys.argv[1:] == ['preflight']:
            preflight(os.environ.get('GITHUB_REF', '') if os.environ.get('GITHUB_REF_TYPE') == 'tag' else '')
            print('Release gate passed')
        else:
            package(sys.argv[1])
    except (ValueError, OSError, subprocess.SubprocessError, IndexError):
        print('Release preparation failed; check approval, configuration, resources and architecture. No diagnostic workspace is uploaded.', file=sys.stderr)
        raise SystemExit(1)
