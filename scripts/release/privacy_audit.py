#!/usr/bin/env python3
"""Offline, value-redacting triage. Never a proof of absence; no network calls."""
import argparse
import collections
import json
import re
import subprocess
from pathlib import Path


def git(*args):
    return subprocess.check_output(['git', '-C', str(Path(__file__).resolve().parents[2]), *args])


PATTERNS = {
    'private_key': rb'-----BEGIN (?:RSA |EC |OPENSSH |DSA |ENCRYPTED )?PRIVATE KEY-----',
    'provider_token': rb'\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,}|AKIA[A-Z0-9]{16}|sk-(?:proj-)?[A-Za-z0-9_-]{32,})\b',
    'literal_secret_candidate': rb'''(?i)(?:password|api[_-]?key|access[_-]?token|pairing[_-]?secret)\s*[=:]\s*[\"'][A-Za-z0-9_+/=-]{16,}[\"']''',
    'credential_url': rb'https?://[^\s/:]+:[^\s/@]+@',
    'personal_home_path': rb'(?:/(?:Users|home)/)[^\s/"<>]+/',
    'email_candidate': rb'\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b',
    'phone_candidate': rb'(?<![\w])\+[1-9][0-9](?:[ ()-]*[0-9]){8,13}(?![\w])',
    'machine_name_candidate': rb'\b[A-Za-z0-9_-]+(?:-MacBook|-Mac-Pro|-iMac)[A-Za-z0-9_.-]*',
}
SENSITIVE = re.compile(r'(?:^|/)(?:\.env(?:\..*)?|id_rsa|id_ed25519|installation\.json|connection\.json|Cookies|Login Data)$|\.(?:sqlite3?|db|p12|pfx|key|mobileprovision|log|crash|dmg|part)$', re.I)
OPAQUE = re.compile(r'\.(?:png|jpe?g|gif|icns|pdf|zip|gz|dylib|a|exe|mp4)$', re.I)


def findings(data, path):
    result = [name for name, pattern in PATTERNS.items() if re.search(pattern, data)]
    if SENSITIVE.search(path):
        result.append('sensitive_filename_candidate')
    if OPAQUE.search(path):
        result.append('opaque_asset_manual_review')
    return result


def scan(history):
    current = collections.defaultdict(set)
    historical = collections.defaultdict(set)
    root=Path(__file__).resolve().parents[2]
    has_git=(root/'.git').exists()
    identities=[]
    if has_git:
        result=subprocess.run(['git','-C',str(root),'log','--all','--format=%an%x00%ae%x00%cn%x00%ce'],capture_output=True)
        if result.returncode==0:identities=result.stdout.decode('utf-8','replace').replace('\n','\0').split('\0')
    personal=[v for v in set(identities) if len(v)>=5 and v not in ['Codex','DLM Development'] and not v.endswith('@users.noreply.github.com')]
    if personal:PATTERNS['personal_identity_candidate']=b'|'.join(re.escape(v.encode()) for v in personal)
    files=git('ls-files','-z').split(b'\0') if has_git else [e['path'].encode() for e in json.loads((root/'SOURCE_MANIFEST.json').read_text())['files']]
    if history and not has_git:raise SystemExit('No public Git history has been initialized')
    for name in filter(None, files):
        path = name.decode('utf-8', 'replace')
        p = root / path
        if p.is_file():
            for category in findings(p.read_bytes(), path):
                current[category].add(path)
    count = 0
    metadata_count = 0
    message_count = 0
    if history:
        commits = git('rev-list', '--all').splitlines()
        count = len(commits)
        for oid in commits:
            identity = git('show', '-s', '--format=%an%x00%ae%x00%cn%x00%ce', oid.decode())
            if identity.strip():
                metadata_count += 1
            message = git('show', '-s', '--format=%B', oid.decode())
            if findings(message, ''):
                message_count += 1
        for path in git('log', '--all', '--format=', '--name-only').decode('utf-8', 'replace').splitlines():
            if SENSITIVE.search(path):
                historical['sensitive_filename_candidate'].add(path)
        proc = subprocess.Popen(['git', 'cat-file', '--batch'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        try:
            for entry in git('rev-list', '--objects', '--all').splitlines():
                oid, _, name = entry.partition(b' ')
                proc.stdin.write(oid + b'\n')
                proc.stdin.flush()
                header = proc.stdout.readline().split()
                data = proc.stdout.read(int(header[2]))
                proc.stdout.read(1)
                if header[1] != b'blob':
                    continue
                path = name.decode('utf-8', 'replace') or '[historical blob]'
                for category in findings(data, path):
                    historical[category].add(path)
        finally:
            proc.stdin.close()
            proc.stdout.close()
            proc.wait()
    # No matched text, identities, remote URLs or configuration values are printed.
    return {'assessment': 'BLOCKED' if current or historical or metadata_count else 'REVIEW_REQUIRED',
            'tracked_files': len(list(filter(None, files))), 'reachable_commits': count,
            'commits_with_identity_metadata_require_review': metadata_count,
            'commit_messages_with_candidates': message_count,
            'current': {k: sorted(v) for k, v in sorted(current.items())},
            'history': {k: sorted(v) for k, v in sorted(historical.items())},
            'limitations': 'Heuristic triage only; emails/fixtures may be public, opaque assets require human review. No entropy or OCR guarantee.'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--history', action='store_true')
    args = parser.parse_args()
    report = scan(args.history)
    print(json.dumps(report, indent=2))
    raise SystemExit(1 if report['assessment'] == 'BLOCKED' else 0)
