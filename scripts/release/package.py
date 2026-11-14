#!/usr/bin/env python3
"""Opt-in headless packaging. No installation, registration or data migration."""
import hashlib
import json
import os
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
NATIVE = ROOT / 'prototypes/swiftui'


# Only inspection enables this private runner-local diagnostic sink.
_INSPECTION_LOG = None


def run(*args, **kwargs):
    if _INSPECTION_LOG is None:
        return subprocess.check_output(args, stderr=subprocess.DEVNULL, **kwargs)
    try:
        output = subprocess.check_output(args, stderr=subprocess.PIPE, **kwargs)
    except subprocess.CalledProcessError as error:
        _INSPECTION_LOG.write(error.output or b'')
        _INSPECTION_LOG.write(error.stderr or b'')
        _INSPECTION_LOG.flush()
        raise
    _INSPECTION_LOG.write(output)
    _INSPECTION_LOG.flush()
    return output


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


def _detach(mount):
    for force in (False, True):
        try:
            args = ['hdiutil', 'detach'] + (['-force'] if force else []) + [str(mount)]
            run(*args)
            return
        except subprocess.SubprocessError:
            if not mount.is_mount():
                return  # Already detached, including a failed attachment.
    raise RuntimeError('Cannot detach inspection mount; workspace retained')


def verify_checksum(dmg, sidecar):
    expected = f'{hashlib.sha256(dmg.read_bytes()).hexdigest()}  {dmg.name}\n'
    if sidecar.read_text() != expected:
        raise ValueError('DMG checksum verification failed')


def _create_verified_dmg(architecture, dist, version):
    """Common mechanics only. Callers separately authorize publication or inspection."""
    if architecture not in ('arm64', 'x86_64') or run('uname', '-m').decode().strip() != architecture:
        raise ValueError('Use a matching native macOS runner')
    app = NATIVE / 'build/DLM.app'
    verify_bundle(app, architecture)
    dist.mkdir(exist_ok=True)
    if dist.is_symlink() or any(dist.iterdir()):
        raise ValueError('Packaging staging must be an empty real directory')
    dmg = dist / f'DLM-{version}-macOS-{architecture}.dmg'
    sidecar = dist / (dmg.name + '.sha256')
    work = dist / '.packaging-work'
    work.mkdir()
    mount = work / 'mounted'
    mount.mkdir()
    attach_attempted = False
    try:
        background = work / 'background.png'
        run('swift', str(ROOT / 'scripts/release/background.swift'),
            str(app / 'Contents/Resources/DLMBrand.png'), str(background))
        env = dict(os.environ, DLM_PACKAGE_APP=str(app), DLM_PACKAGE_BACKGROUND=str(background))
        run('dmgbuild', '-s', str(ROOT / 'scripts/release/dmg_settings.py'), f'DLM {version}', str(dmg), env=env)
        run('hdiutil', 'verify', str(dmg))
        attach_attempted = True
        run('hdiutil', 'attach', '-readonly', '-nobrowse', '-mountpoint', str(mount), str(dmg))
        if not (mount / 'Applications').is_symlink() or os.readlink(mount / 'Applications') != '/Applications':
            raise ValueError('DMG Applications shortcut missing')
        allowed = {'DLM.app', 'Applications', '.DS_Store', '.background', '.VolumeIcon.icns', '.fseventsd', '.Trashes'}
        if not {p.name for p in mount.iterdir()} <= allowed:
            raise ValueError('Unexpected DMG content')
        if not (mount / '.DS_Store').is_file():
            raise ValueError('DMG Finder presentation missing')
        verify_bundle(mount / 'DLM.app', architecture)
    finally:
        if attach_attempted:
            _detach(mount)
        # Never walk/remove a filesystem that is still mounted after cleanup failure.
        if mount.is_mount():
            raise RuntimeError('Packaging mount remains; workspace retained')
        shutil.rmtree(work)
    digest = hashlib.sha256(dmg.read_bytes()).hexdigest()
    sidecar.write_text(f'{digest}  {dmg.name}\n')
    verify_checksum(dmg, sidecar)
    return dmg, sidecar


def package(architecture):
    version, _ = preflight()  # Publication remains fail-closed before any packaging.
    dmg, sidecar = _create_verified_dmg(architecture, ROOT / 'dist', version)
    if os.environ.get('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
            output.write(f'dmg=dist/{dmg.name}\nchecksum=dist/{sidecar.name}\n')
    print(dmg.name)


def _inspection_root():
    if (os.environ.get('GITHUB_ACTIONS') != 'true'
            or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted'
            or os.environ.get('GITHUB_EVENT_NAME') != 'workflow_dispatch'):
        raise ValueError('Inspection requires a manually dispatched GitHub-hosted run')
    raw = os.environ.get('RUNNER_TEMP', '')
    root = Path(raw)
    if not raw or not root.is_absolute() or not root.is_dir() or root.is_symlink():
        raise ValueError('Invalid runner temporary directory')
    root = root.resolve()
    if root == ROOT.resolve() or root.is_relative_to(ROOT.resolve()):
        raise ValueError('Inspection output must be outside the checkout')
    return root


_INSPECTION_MARKER = '.dlm-inspection-owner'
_INSPECTION_OWNER = 'DLM runner-local inspection v1\n'


def _remove_inspection(directory):
    marker = directory / _INSPECTION_MARKER
    if directory.is_symlink() or marker.is_symlink() or not marker.is_file() or marker.read_text() != _INSPECTION_OWNER:
        raise ValueError('Unrecognized inspection workspace; refusing cleanup')
    images = directory / 'images'
    mount = images / '.packaging-work/mounted'
    if any(p.is_symlink() for p in (images, images / '.packaging-work', mount)):
        raise ValueError('Unexpected workspace symlink; refusing cleanup')
    if mount.is_mount():
        _detach(mount)
    if mount.is_mount():
        raise RuntimeError('Mount remains; inspection files retained for runner teardown')
    shutil.rmtree(directory)


def inspect_runner_local(architecture):
    root = _inspection_root()
    version, _ = metadata()
    directory = Path(tempfile.mkdtemp(prefix='dlm-dmg-inspection-', dir=root))
    (directory / _INSPECTION_MARKER).write_text(_INSPECTION_OWNER)
    images = directory / 'images'
    images.mkdir()
    # NamedTemporaryFile creates a private file; its name is never printed or uploaded.
    global _INSPECTION_LOG
    previous = _INSPECTION_LOG
    try:
        with tempfile.NamedTemporaryFile(prefix='dlm-dmg-diagnostic-', suffix='.log', dir=root, delete=False) as log:
            _INSPECTION_LOG = log
            try:
                _create_verified_dmg(architecture, images, version)
                # No GITHUB_OUTPUT, source dist directory or publication callback is used.
            finally:
                _remove_inspection(directory)
    finally:
        _INSPECTION_LOG = previous
    print('Runner-local DMG checks passed; temporary image and checksum discarded.')


def cleanup_inspections():
    root = _inspection_root()
    for directory in root.glob('dlm-dmg-inspection-*'):
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError('Unexpected inspection workspace')
        _remove_inspection(directory)
    print('Runner-local inspection cleanup completed.')


if __name__ == '__main__':
    try:
        args = sys.argv[1:]
        if args == ['preflight']:
            preflight(os.environ.get('GITHUB_REF', '') if os.environ.get('GITHUB_REF_TYPE') == 'tag' else '')
            print('Release gate passed')
        elif len(args) == 2 and args[0] == 'inspect-runner-local':
            inspect_runner_local(args[1])
        elif args == ['cleanup-inspections']:
            cleanup_inspections()
        elif len(args) == 1 and args[0] in ('arm64', 'x86_64'):
            package(args[0])
        else:
            raise ValueError('Unsupported packaging command')
    except (ValueError, OSError, subprocess.SubprocessError, IndexError, KeyError, TypeError, RuntimeError):
        print('Packaging verification failed; check runner-local diagnostics and configuration. Nothing is uploaded by this command.', file=sys.stderr)
        raise SystemExit(1)
