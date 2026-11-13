#!/usr/bin/env python3
"""Verify only the exact two DMGs and checksum sidecars from this workflow run."""
import hashlib
import re
from pathlib import Path
from package import metadata


def verify(directory):
    version, _ = metadata()
    names = {f'DLM-{version}-macOS-{arch}.dmg' for arch in ['arm64', 'x86_64']}
    expected = names | {name + '.sha256' for name in names}
    if {p.name for p in directory.iterdir()} != expected:
        raise ValueError('Unexpected or missing release artifact')
    for name in names:
        dmg = directory / name
        sidecar = directory / (name + '.sha256')
        if dmg.is_symlink() or sidecar.is_symlink() or not dmg.is_file():
            raise ValueError('Artifacts must be regular files')
        match = re.fullmatch(r'([a-f0-9]{64})  ' + re.escape(name) + r'\n', sidecar.read_text())
        if not match or hashlib.sha256(dmg.read_bytes()).hexdigest() != match[1]:
            raise ValueError('Release checksum mismatch')
    return sorted(expected)


if __name__ == '__main__':
    verify(Path('dist'))
    print('Both architecture artifact checksums verified')
