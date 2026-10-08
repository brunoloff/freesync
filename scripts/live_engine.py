#!/usr/bin/env python3
"""Continuous-engine evidence in the authorized test pair; no credentials in output."""
import argparse
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
from live_transfers import CLI, FIXTURE, PROFILE, REPO, baseline, call, state

DESKTOP_SOAK = False


def wait_for(predicate, timeout=240):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return
        except (StopIteration, sqlite3.OperationalError, FileNotFoundError):
            pass
        time.sleep(0.5)
    raise RuntimeError('A monitored engine check did not converge.')


def status():
    values = call(CLI, 'status')['statuses']
    return next((s for s in values if s and s['pair_id'] == 'test-freesync'), {})


def engine(offline=False):
    env = os.environ.copy()
    if offline:
        env['HTTPS_PROXY'] = 'http://127.0.0.1:9'
        env['HTTP_PROXY'] = 'http://127.0.0.1:9'
        env['ALL_PROXY'] = 'http://127.0.0.1:9'
        env['NO_PROXY'] = ''
    if DESKTOP_SOAK and not offline:
        env['FREESYNC_BROWSER_TEST'] = '1'
        command = [str(REPO / 'target/debug/freesync-desktop')]
    else:
        command = [str(CLI), 'run']
    return subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)


def stop(process):
    if process.poll() is None:
        call(CLI, 'quit')
    process.communicate(timeout=90)
    if process.returncode:
        raise RuntimeError('The engine did not stop cleanly.')


def converged(path, content):
    expected = hashlib.md5(content).hexdigest()
    return baseline(path)['local']['fingerprint']['md5'] == expected and baseline(path)['remote']['fingerprint']['md5'] == expected and status().get('queued') == 0


def main():
    global DESKTOP_SOAK
    parser = argparse.ArgumentParser()
    parser.add_argument('--soak-hours', type=float, default=0)
    parser.add_argument('--desktop-soak', action='store_true', help='Use the actual desktop owner during soak, after the CLI sequence.')
    args = parser.parse_args()
    mapping = json.loads((Path.home() / '.config/freesync/development.json').read_text())
    pair = mapping['test_pair']
    root = Path(pair['local_root'])
    if root.name != 'test-freesync' or not root.is_absolute() or subprocess.run(['pgrep', '-f', '[i]nsync'], capture_output=True).returncode == 0:
        raise RuntimeError('The authorized isolated test root is not available.')
    run = 'run-' + uuid.uuid4().hex[:12]
    local = root / run
    local.mkdir()
    evidence = {'run': run, 'started_at': time.time(), 'checks': {}, 'observations': [], 'restarts': 0}
    report = PROFILE / ('live-engine-' + run + '.json')

    def save():
        report.write_text(json.dumps(evidence, indent=2))
        os.chmod(report, 0o600)

    def passed(name):
        evidence['checks'][name] = True
        save()
        print(json.dumps({'check': name, 'passed': True}), flush=True)

    call(CLI, 'activate')
    call(CLI, 'resume')
    process = engine()
    try:
        (local / 'local.txt').write_bytes(b'automatic local creation\n')
        wait_for(lambda: converged(run + '/local.txt', b'automatic local creation\n'))
        folder = baseline(run)['remote']
        first = baseline(run + '/local.txt')['remote']
        with tempfile.TemporaryDirectory(prefix='freesync-engine-input-') as temporary:
            source = Path(temporary) / 'source'
            download = Path(temporary) / 'download'
            source.write_bytes(b'automatic remote creation\n')
            remote = call(FIXTURE, 'upload', folder['id'], 'remote.txt', source)
            wait_for(lambda: (local / 'remote.txt').exists() and converged(run + '/remote.txt', source.read_bytes()))
            assert (local / 'remote.txt').read_bytes() == source.read_bytes()
            call(FIXTURE, 'download', first['id'], download)
            assert download.read_bytes() == (local / 'local.txt').read_bytes()
            passed('automatic_bidirectional_creation_with_verified_content')

            (local / 'local.txt').write_bytes(b'automatic local edit\n')
            wait_for(lambda: converged(run + '/local.txt', b'automatic local edit\n'))
            source.write_bytes(b'automatic remote edit\n')
            call(FIXTURE, 'upload', folder['id'], 'remote.txt', source, remote['id'])
            wait_for(lambda: converged(run + '/remote.txt', source.read_bytes()))
            assert (local / 'remote.txt').read_bytes() == source.read_bytes()
            (local / 'local.txt').rename(local / 'moved.txt')
            wait_for(lambda: any(b['path'] == run + '/moved.txt' and b['remote']['id'] == first['id'] for b in state('baselines')))
            call(FIXTURE, 'move', remote['id'], folder['id'], 'remote-moved.txt')
            wait_for(lambda: (local / 'remote-moved.txt').exists() and not (local / 'remote.txt').exists())
            assert call(FIXTURE, 'inspect', first['id'])['name'] == 'moved.txt'
            passed('automatic_edits_and_moves_keep_persisted_identities')

            (local / 'delete-local').write_bytes(b'local deletion fixture')
            wait_for(lambda: converged(run + '/delete-local', b'local deletion fixture'))
            deleted = baseline(run + '/delete-local')['remote']
            (local / 'delete-local').unlink()
            wait_for(lambda: not any(b['path'] == run + '/delete-local' for b in state('baselines')) and status().get('queued') == 0)
            assert call(FIXTURE, 'inspect', deleted['id'])['trashed']
            call(FIXTURE, 'trash', remote['id'])
            wait_for(lambda: not (local / 'remote-moved.txt').exists() and status().get('queued') == 0)
            assert any((p / 'content').is_file() and (p / 'content').read_bytes() == source.read_bytes() for p in (PROFILE / 'recovery').iterdir())
            passed('automatic_deletions_retain_drive_trash_and_local_recovery')

            call(CLI, 'pause')
            wait_for(lambda: status().get('state') == 'paused')
            (local / 'moved.txt').write_bytes(b'simultaneous local version\n')
            source.write_bytes(b'simultaneous remote version\n')
            call(FIXTURE, 'upload', folder['id'], 'moved.txt', source, first['id'])
            assert (local / 'moved.txt').read_bytes() != source.read_bytes()
            call(CLI, 'resume')
            wait_for(lambda: any(c['path'] == run + '/moved.txt' for c in state('conflicts')))
            (local / 'unrelated.txt').write_bytes(b'unrelated paths continue\n')
            wait_for(lambda: converged(run + '/unrelated.txt', b'unrelated paths continue\n'))
            call(FIXTURE, 'download', first['id'], download)
            assert download.read_bytes() == source.read_bytes()
            assert (local / 'moved.txt').read_bytes() == b'simultaneous local version\n'
            passed('pause_resume_and_visible_conflict_do_not_stall_unrelated_files')

            stop(process)
            (local / 'offline.txt').write_bytes(b'change while network unavailable\n')
            process = engine(offline=True)
            wait_for(lambda: status().get('state') == 'offline', timeout=120)
            assert not any(b['path'] == run + '/offline.txt' for b in state('baselines'))
            assert (local / 'offline.txt').read_bytes() == b'change while network unavailable\n'
            stop(process)
            process = engine()
            evidence['restarts'] += 2
            wait_for(lambda: converged(run + '/offline.txt', b'change while network unavailable\n'))
            uploaded = baseline(run + '/offline.txt')['remote']
            call(FIXTURE, 'download', uploaded['id'], download)
            assert download.read_bytes() == (local / 'offline.txt').read_bytes()
            passed('actual_network_failure_and_process_restarts_preserve_and_converge_changes')

            # A fresh steady-state snapshot must agree; our writes must not echo.
            count = len(state('operations'))
            call(CLI, 'sync-now')
            time.sleep(pair['poll_secs'] * 2 + 2)
            wait_for(lambda: status().get('queued') == 0)
            assert len(state('operations')) == count
            passed('stable_empty_queue_without_feedback_operations')

            if args.soak_hours:
                if args.desktop_soak:
                    wait_for(lambda: (REPO / 'target/debug/freesync-desktop').is_file(), timeout=1800)
                    stop(process)
                    DESKTOP_SOAK = True
                    process = engine()
                    wait_for(lambda: status().get('state') in ('idle', 'conflicts'))
                    evidence['desktop_owner'] = True
                    save()
                    print(json.dumps({'desktop_soak_owner_started': True}), flush=True)
                duration = args.soak_hours * 3600
                start = time.monotonic()
                next_change = start
                next_restart = start + duration / 3
                tick = 0
                evidence['soak_started_at'] = time.time()
                save()
                while time.monotonic() - start < duration:
                    if process.poll() is not None:
                        expected = Path('/tmp/freesync-soak-expected-restart')
                        if args.desktop_soak and expected.is_file() and process.returncode == 0:
                            expected.unlink()
                            process.communicate()
                            process = engine()
                            evidence['restarts'] += 1
                            evidence['ui_quit_restart_verified'] = True
                            wait_for(lambda: status().get('state') in ('idle', 'conflicts'))
                            save()
                        else:
                            raise RuntimeError('The monitored engine exited during soak.')
                    if time.monotonic() >= next_change:
                        content = f'monitored soak checkpoint {tick}\n'.encode()
                        if tick % 2 == 0:
                            (local / 'soak.txt').write_bytes(content)
                        else:
                            item = baseline(run + '/soak.txt')['remote']
                            source.write_bytes(content)
                            call(FIXTURE, 'upload', folder['id'], 'soak.txt', source, item['id'])
                        wait_for(lambda: converged(run + '/soak.txt', content))
                        assert (local / 'soak.txt').read_bytes() == content
                        item = baseline(run + '/soak.txt')['remote']
                        call(FIXTURE, 'download', item['id'], download)
                        assert download.read_bytes() == content
                        evidence['observations'].append({'at': time.time(), 'checkpoint': tick, 'direction': 'local' if tick % 2 == 0 else 'remote', 'content_verified': True, 'queue': status().get('queued'), 'conflicts_retained': any(c['path'] == run + '/moved.txt' for c in state('conflicts'))})
                        save()
                        print(json.dumps({'soak_checkpoint': tick, 'content_verified': True, 'queue': 0}), flush=True)
                        tick += 1
                        next_change = time.monotonic() + 600
                    if time.monotonic() >= next_restart and evidence['restarts'] < 4:
                        stop(process)
                        process = engine()
                        evidence['restarts'] += 1
                        next_restart += duration / 3
                        wait_for(lambda: status().get('queued') == 0 and status().get('state') in ('idle', 'conflicts'))
                        save()
                        print(json.dumps({'soak_restart': evidence['restarts'] - 2, 'queue': 0}), flush=True)
                    current = status()
                    if current.get('state') in ('error', 'reconnect', 'needs_review'):
                        raise RuntimeError('The engine needs attention during soak.')
                    time.sleep(30)
                evidence['soak_elapsed_seconds'] = time.monotonic() - start
                assert len(evidence['observations']) >= 3 and evidence['restarts'] >= 4
                passed('multi_hour_monitored_soak_with_verified_changes_and_multiple_restarts')
        evidence['finished_at'] = time.time()
        save()
        print(json.dumps({'live_engine_suite_passed': True, 'checks': len(evidence['checks']), 'fixture': run, 'soak_hours': args.soak_hours}), flush=True)
    finally:
        stop(process)


if __name__ == '__main__':
    main()
