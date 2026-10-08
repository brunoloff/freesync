#!/usr/bin/env python3
"""Supervise the actual desktop owner independently of terminal session lifetime.

Reuse a completed automatic-sequence fixture. Each checkpoint verifies both
persisted Drive bytes and disk bytes. Evidence is private; elapsed time alone
cannot pass this check. Run with a detached session and redirected private logs.
"""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time

from live_engine import converged, status, wait_for
from live_transfers import CLI, FIXTURE, PROFILE, REPO, baseline, call, state


def save(report, evidence):
    temporary = report.with_suffix('.tmp')
    with open(temporary, 'w', encoding='utf-8') as output:
        os.chmod(temporary, 0o600)
        json.dump(evidence, output, indent=2)
        output.flush()
        os.fsync(output.fileno())
    temporary.replace(report)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--run', required=True)
    parser.add_argument('--hours', type=float, default=3)
    args = parser.parse_args()
    if not re.fullmatch(r'run-[0-9a-f]{12}', args.run) or args.hours < 3:
        raise RuntimeError('Use an existing disposable fixture and at least three hours.')
    mapping = json.loads((Path.home() / '.config/freesync/development.json').read_text())
    root = Path(mapping['test_pair']['local_root'])
    if not root.is_absolute() or root.name != 'test-freesync':
        raise RuntimeError('The authorized isolated test root is unavailable.')
    previous = json.loads((PROFILE / ('live-engine-' + args.run + '.json')).read_text())
    if len(previous['checks']) < 6 or not all(previous['checks'].values()):
        raise RuntimeError('The automatic sequence must pass before this soak.')
    local = root / args.run
    if not local.is_dir() or local.is_symlink():
        raise RuntimeError('The existing disposable fixture is unavailable.')
    # The mutation adapter separately revalidates the private mapping/ancestry.
    folder = baseline(args.run)['remote']
    conflict_path = args.run + '/moved.txt'
    report = PROFILE / ('live-soak-' + args.run + '.json')
    evidence = {'run': args.run, 'started_at': time.time(), 'supervisor_pid': os.getpid(),
                'hours_required': args.hours, 'observations': [], 'restarts': [],
                'passed': False, 'desktop_owner': True}
    save(report, evidence)
    stopping = False

    def stop_requested(_number, _frame):
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGTERM, stop_requested)
    signal.signal(signal.SIGINT, stop_requested)
    process = None
    engine_log = open(PROFILE / 'soak-desktop.log', 'ab', buffering=0)
    os.chmod(PROFILE / 'soak-desktop.log', 0o600)

    def ensure_isolated():
        if subprocess.run(['pgrep', '-f', '[i]nsync'], capture_output=True).returncode == 0:
            raise RuntimeError('Another sync process is running. Stop the isolated test.')

    def launch():
        ensure_isolated()
        environment = os.environ.copy()
        environment['FREESYNC_BROWSER_TEST'] = '1'
        owner = subprocess.Popen([str(REPO / 'target/debug/freesync-desktop')],
                                 env=environment, stdout=engine_log, stderr=engine_log)
        wait_for(lambda: owner.poll() is not None or status().get('state') in ('idle', 'conflicts'))
        if owner.poll() is not None:
            raise RuntimeError('The desktop owner could not start.')
        evidence['owner_pid'] = owner.pid
        save(report, evidence)
        return owner

    def stop_owner():
        if process and process.poll() is None:
            call(CLI, 'quit')
            process.wait(timeout=90)
        if process and process.returncode != 0:
            raise RuntimeError('The desktop owner did not stop cleanly.')

    try:
        call(CLI, 'resume')
        process = launch()
        start = time.monotonic()
        duration = args.hours * 3600
        next_change = start
        restart_times = [start + duration / 3, start + duration * 2 / 3]
        tick = 0
        with tempfile.TemporaryDirectory(prefix='freesync-soak-input-') as temporary:
            source, download = Path(temporary) / 'source', Path(temporary) / 'download'
            while time.monotonic() - start < duration and not stopping:
                ensure_isolated()
                if process.poll() is not None:
                    marker = Path('/tmp/freesync-soak-expected-restart')
                    if not marker.is_file() or process.returncode != 0:
                        raise RuntimeError('The monitored desktop owner exited unexpectedly.')
                    marker.unlink()
                    process = launch()
                    evidence['restarts'].append({'at': time.time(), 'reason': 'ui_quit', 'clean': True})
                if time.monotonic() >= next_change:
                    content = f'desktop soak checkpoint {tick}\n'.encode()
                    if tick % 2 == 0:
                        (local / 'soak.txt').write_bytes(content)
                    else:
                        item = baseline(args.run + '/soak.txt')['remote']
                        source.write_bytes(content)
                        call(FIXTURE, 'upload', folder['id'], 'soak.txt', source, item['id'])
                    wait_for(lambda: converged(args.run + '/soak.txt', content))
                    assert (local / 'soak.txt').read_bytes() == content
                    item = baseline(args.run + '/soak.txt')['remote']
                    call(FIXTURE, 'download', item['id'], download)
                    assert download.read_bytes() == content
                    retained = any(c['path'] == conflict_path for c in state('conflicts'))
                    assert retained and status().get('queued') == 0
                    evidence['observations'].append({'at': time.time(), 'checkpoint': tick,
                        'direction': 'local' if tick % 2 == 0 else 'remote',
                        'content_verified': True, 'queue': 0, 'conflicts_retained': retained})
                    print(json.dumps({'checkpoint': tick, 'content_verified': True, 'queue': 0}), flush=True)
                    tick += 1
                    next_change = time.monotonic() + 600
                if restart_times and time.monotonic() >= restart_times[0]:
                    restart_times.pop(0)
                    stop_owner()
                    process = launch()
                    evidence['restarts'].append({'at': time.time(), 'reason': 'scheduled', 'clean': True})
                current = status()
                if current.get('state') in ('error', 'reconnect', 'needs_review'):
                    raise RuntimeError('The desktop engine needs attention during soak.')
                evidence['last_heartbeat'] = time.time()
                evidence['elapsed_seconds'] = time.monotonic() - start
                evidence['queue'] = current.get('queued')
                evidence['state'] = current.get('state')
                save(report, evidence)
                for _ in range(30):
                    if stopping:
                        break
                    time.sleep(1)
        if stopping:
            raise RuntimeError('The monitored test was stopped before completion.')
        stop_owner()
        assert len(evidence['observations']) >= 3 and len(evidence['restarts']) >= 2
        evidence['passed'] = True
        evidence['finished_at'] = time.time()
        evidence['elapsed_seconds'] = time.monotonic() - start
        evidence['graceful_final_quit'] = True
        save(report, evidence)
        print(json.dumps({'multi_hour_desktop_soak_passed': True,
                          'checkpoints': len(evidence['observations']),
                          'restarts': len(evidence['restarts'])}), flush=True)
    except BaseException:
        evidence['failed_at'] = time.time()
        evidence['failure'] = 'The monitored test did not finish. Pending work and fixtures remain preserved.'
        save(report, evidence)
        raise
    finally:
        stop_owner()
        engine_log.close()


if __name__ == '__main__':
    main()
