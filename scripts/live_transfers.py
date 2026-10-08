#!/usr/bin/env python3
"""Authorized integration checks. All synced mutations remain in test-freesync.
Credential values and upload URLs never enter this report or subprocess diagnostics.
"""
import hashlib
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import tempfile
import time
import uuid

REPO = Path(__file__).resolve().parents[1]
CLI = REPO / 'target/debug/freesync'
FIXTURE = REPO / 'target/debug/examples/fixture'
PROFILE = Path.home() / '.local/share/freesync/profiles/default'


def call(binary, *args):
    result = subprocess.run([str(binary), *map(str, args)], capture_output=True, text=True, timeout=240)
    try:
        value = json.loads(result.stdout or result.stderr)
    except ValueError:
        raise RuntimeError('A test command did not return valid JSON.') from None
    if result.returncode:
        raise RuntimeError('A test command failed: ' + value.get('error', {}).get('code', 'unknown'))
    return value


def state(table):
    with sqlite3.connect(f'file:{PROFILE / "state.sqlite3"}?mode=ro', uri=True) as db:
        return [json.loads(r[0]) for r in db.execute(f'SELECT json FROM {table} WHERE pair=?', ('test-freesync',))]


def settle():
    for _ in range(12):
        result = call(CLI, 'sync-once')
        if result['queued'] == 0:
            return result
        time.sleep(2)
    raise RuntimeError('The durable queue did not settle.')


def baseline(path):
    return next(b for b in state('baselines') if b['path'] == path)


def interrupt_upload():
    process = subprocess.Popen([str(CLI), 'sync-once', '--chunk-size', '262144'], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    deadline = time.monotonic() + 180
    partial = None
    while time.monotonic() < deadline:
        if process.poll() is not None:
            process.communicate()
            raise RuntimeError('The upload finished before an interruption could be verified.')
        operations = state('operations')
        partial = next((op for op in operations if op['state'] == 'running' and (op.get('upload_session') or {}).get('uploaded_bytes', 0) > 0), None)
        if partial:
            process.send_signal(signal.SIGINT)
            process.communicate(timeout=60)
            return partial['upload_session']['uploaded_bytes']
        time.sleep(0.15)
    process.send_signal(signal.SIGINT)
    process.communicate(timeout=60)
    raise RuntimeError('No durable partial-upload checkpoint was observed.')


def main():
    mapping = json.loads((Path.home() / '.config/freesync/development.json').read_text())
    pair = mapping['test_pair']
    root = Path(pair['local_root'])
    if root.name != 'test-freesync' or not root.is_absolute():
        raise RuntimeError('The expected disposable local root is not configured.')
    if subprocess.run(['pgrep', '-f', '[i]nsync'], capture_output=True).returncode == 0:
        raise RuntimeError('InSync is still running.')
    run = 'run-' + uuid.uuid4().hex[:12]
    local = root / run
    local.mkdir()
    evidence = {'run': run, 'started_at': time.time(), 'checks': {}}
    report = PROFILE / ('live-transfers-' + run + '.json')

    def passed(name):
        evidence['checks'][name] = True
        report.write_text(json.dumps(evidence, indent=2))
        os.chmod(report, 0o600)
        print(json.dumps({'check': name, 'passed': True}), flush=True)

    folder = call(FIXTURE, 'folder', pair['remote_root_id'], run)
    call(CLI, 'check-writes', '--id', folder['id'])
    passed('server_rejects_stale_conditional_write')
    with tempfile.TemporaryDirectory(prefix='freesync-live-input-') as temporary:
        temporary = Path(temporary)
        remote_source = temporary / 'remote.txt'
        remote_source.write_bytes(b'created remotely for FreeSync integration\n')
        zero_source = temporary / 'zero'
        zero_source.write_bytes(b'')
        remote_file = call(FIXTURE, 'upload', folder['id'], 'remote.txt', remote_source)
        call(FIXTURE, 'upload', folder['id'], 'remote-zero', zero_source)
        (local / 'local.txt').write_bytes(b'created locally for FreeSync integration\n')
        (local / 'local-zero').write_bytes(b'')
        settled = settle()
        assert settled['conflicts'] == 0
        assert (local / 'remote.txt').read_bytes() == remote_source.read_bytes()
        assert (local / 'remote-zero').read_bytes() == b''
        local_file = baseline(run + '/local.txt')['remote']
        downloaded = temporary / 'downloaded'
        call(FIXTURE, 'download', local_file['id'], downloaded)
        assert downloaded.read_bytes() == (local / 'local.txt').read_bytes()
        assert baseline(run + '/local-zero')['remote']['fingerprint']['size'] == 0
        passed('bidirectional_content_and_zero_byte_files')

        (local / 'local.txt').write_bytes(b'local edit propagated to Drive\n')
        settle()
        call(FIXTURE, 'download', local_file['id'], downloaded)
        assert downloaded.read_bytes() == (local / 'local.txt').read_bytes()
        remote_source.write_bytes(b'remote edit propagated to disk\n')
        call(FIXTURE, 'upload', folder['id'], 'remote.txt', remote_source, remote_file['id'])
        settle()
        assert (local / 'remote.txt').read_bytes() == remote_source.read_bytes()
        passed('conditional_existing_file_updates_both_directions')

        (local / 'large.bin').write_bytes(bytes(range(256)) * 32768 + b'end')
        checkpoint = interrupt_upload()
        assert 0 < checkpoint < (local / 'large.bin').stat().st_size
        settle()
        large = baseline(run + '/large.bin')['remote']
        call(FIXTURE, 'download', large['id'], downloaded)
        assert hashlib.sha256(downloaded.read_bytes()).digest() == hashlib.sha256((local / 'large.bin').read_bytes()).digest()
        inventory = call(CLI, 'remote-scan', '--root-id', folder['id'])
        assert len(inventory['entries']['large.bin']) == 1
        passed('interrupted_large_upload_resumes_after_process_restart_without_duplicates')

        (local / 'local.txt').rename(local / 'renamed.txt')
        settle()
        persisted = call(FIXTURE, 'inspect', local_file['id'])
        assert persisted['name'] == 'renamed.txt'
        call(FIXTURE, 'move', remote_file['id'], folder['id'], 'remote-renamed.txt')
        settle()
        assert (local / 'remote-renamed.txt').exists() and not (local / 'remote.txt').exists()
        passed('local_and_remote_moves_preserve_file_ids')

        (local / 'local-zero').unlink()
        settle()
        deleted_id = next(o['expected_remote']['id'] for o in state('operations') if o['path'] == run + '/local-zero' and o['action']['kind'] == 'trash_remote')
        assert call(FIXTURE, 'inspect', deleted_id)['trashed']
        call(FIXTURE, 'trash', remote_file['id'])
        settle()
        assert not (local / 'remote-renamed.txt').exists()
        assert any((p / 'content').is_file() and (p / 'content').read_bytes() == remote_source.read_bytes() for p in (PROFILE / 'recovery').iterdir())
        passed('persisted_drive_trash_and_local_recovery')

        (local / 'large.bin').write_bytes(bytes([42]) * (8 * 1024 * 1024 + 3))
        interrupt_upload()
        remote_source.write_bytes(b'intervening remote edit preserved during resume\n')
        call(FIXTURE, 'upload', folder['id'], 'large.bin', remote_source, large['id'])
        settle()
        call(FIXTURE, 'download', large['id'], downloaded)
        assert downloaded.read_bytes() == remote_source.read_bytes()
        assert (local / 'large.bin').read_bytes() == bytes([42]) * (8 * 1024 * 1024 + 3)
        assert any(c['path'] == run + '/large.bin' for c in state('conflicts'))
        passed('intervening_edit_survives_interrupted_update_as_visible_conflict')
        settled = settle()
        assert settled['queued'] == 0 and settled['completed'] == 0
        passed('unchanged_queue_is_stable_and_conflict_remains_visible')
    evidence['finished_at'] = time.time()
    report.write_text(json.dumps(evidence, indent=2))
    print(json.dumps({'live_transfer_suite_passed': True, 'checks': len(evidence['checks']), 'fixture': run}), flush=True)


if __name__ == '__main__':
    main()
