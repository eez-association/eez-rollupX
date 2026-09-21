#!/usr/bin/env python3
"""Physical 3GiB ext4 quota and lagging-reader drill, owned disposable Docker only.

The native Publisher/Writer and bounded reader are real; protocol progress and
production storage provisioning are deliberately not inferred from this fixture.
"""
import argparse
import errno
import hashlib
import json
import os
import pathlib
import re
import select
import signal
import shutil
import subprocess
import tempfile
import time
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
CAPACITY = 3 * 1024**3


def run(args, **kwargs):
    return subprocess.run(args, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                          timeout=120, check=True, **kwargs).stdout


def until(label, callback, seconds=20):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = callback()
        if result:
            return result
        time.sleep(.05)
    raise AssertionError(label + ' did not become true')


def inode_fds(pid, inode):
    result = []
    for path in pathlib.Path(f'/proc/{pid}/fd').iterdir():
        try:
            metadata = path.stat()
            if (metadata.st_dev, metadata.st_ino) == inode:
                result.append(str(path))
        except FileNotFoundError:
            pass
    return result


def inside():
    report = {'checks': [], 'image_capacity_bytes': CAPACITY, 'scope': 'physical filesystem and actual telemetry transport only; no protocol-progress or overhead claim'}
    report['mounted_binary_sha256'] = hashlib.sha256(pathlib.Path('/fixture/spool-fixture').read_bytes()).hexdigest()
    fixture = alloy = None
    ballast = lagger = None
    mounted = False
    def note(label, **fields):
        report['checks'].append(label)
        report.update(fields)
        print('PASS: ' + label, flush=True)
    def physical():
        values = os.statvfs('/quota')
        return {'capacity_bytes': values.f_blocks * values.f_frsize,
                'free_bytes': values.f_bfree * values.f_frsize,
                'available_bytes': values.f_bavail * values.f_frsize,
                'image_bytes': os.stat('/volume.ext4').st_size,
                'image_allocated_bytes': os.stat('/volume.ext4').st_blocks * 512}
    def request(command, **values):
        fixture.stdin.write(json.dumps({'command': command, **values}) + '\n')
        fixture.stdin.flush()
        assert select.select([fixture.stdout], [], [], 10)[0], 'native fixture command blocked'
        response = fixture.stdout.readline()
        assert response, 'native fixture exited unexpectedly'
        return json.loads(response)
    try:
        # The image is created only in this disposable container's private layer.
        backing = os.statvfs('/')
        report['backing_available_bytes'] = backing.f_bavail * backing.f_frsize
        assert report['backing_available_bytes'] > CAPACITY * 2, 'insufficient disposable backing storage'
        with open('/volume.ext4', 'wb') as image:
            image.truncate(CAPACITY)
        run(['mkfs.ext4', '-q', '-F', '-m', '0', '-b', '4096', '-N', '65536', '/volume.ext4'])
        pathlib.Path('/quota').mkdir()
        run(['fuse2fs', '/volume.ext4', '/quota'])
        mounted = True
        report['filesystem_tools'] = run(['dpkg-query','-W','fuse2fs','e2fsprogs','fuse3','libext2fs2t64','libfuse3-4'])
        report['filesystem_initial'] = physical()
        assert report['filesystem_initial']['capacity_bytes'] <= CAPACITY
        assert report['filesystem_initial']['available_bytes'] > 2 * 1024**3
        run(['alloy', 'validate', '/fixture/alloy.alloy'])
        fixture = subprocess.Popen(['/fixture/spool-fixture', '/quota/spool'], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=open('/fixture-stderr.log', 'w'), text=True, bufsize=1)
        assert json.loads(fixture.stdout.readline()) == {'ready': True}
        report['native_pid'] = fixture.pid
        first = next(pathlib.Path('/quota/spool').glob('*.active'))
        lagger = open(first, 'rb')
        first_metadata = os.fstat(lagger.fileno())
        first_inode = (first_metadata.st_dev, first_metadata.st_ino)
        before_rotation_read = request('read')
        assert before_rotation_read['descriptors_before'] == before_rotation_read['descriptors_after']
        report['reader_before_rotation'] = before_rotation_read
        alloy = subprocess.Popen(['alloy', 'run', '--disable-reporting', '--storage.path=/alloy-data',
                                  '--server.http.listen-addr=127.0.0.1:12345', '/fixture/alloy.alloy'],
                                 stdout=open('/alloy.log', 'w'), stderr=subprocess.STDOUT)
        until('Alloy opened active lifecycle segment', lambda: inode_fds(alloy.pid, first_inode))
        os.kill(alloy.pid, signal.SIGSTOP)
        report['alloy_pid'] = alloy.pid
        published = request('publish', count=2000)
        until('writer drained initial records', lambda: request('stats')['stats']['written_records'] + request('stats')['stats']['dropped_records'] >= published['calls'])
        until('retention unlinked original active inode', lambda: os.fstat(lagger.fileno()).st_nlink == 0)
        assert inode_fds(alloy.pid, first_inode), 'deliberately stopped Alloy lost the held inode'
        assert not inode_fds(fixture.pid, first_inode), 'native reader pinned the segment across rotation'
        visible = sum(path.stat().st_size for path in pathlib.Path('/quota/spool').iterdir() if path.suffix in ('.active', '.closed'))
        assert visible <= 131072, visible
        first_physical = physical()
        pinned_bytes = os.fstat(lagger.fileno()).st_blocks * 512
        assert pinned_bytes > 0
        note('rotation_retention_with_two_deliberately_lagging_descriptors', visible_retained_bytes=visible,
             held_unlinked_segment_bytes=pinned_bytes, unlinked_segment_links=os.fstat(lagger.fileno()).st_nlink,
             physical_after_rotation=first_physical)
        read = request('read')
        assert read['descriptors_before'] == read['descriptors_after']
        assert read['lines'] > 0
        note('native_bounded_reader_closes_descriptors_while_returned_buffers_live', reader_pass=read)
        ballast = open('/quota/lagging-ballast', 'wb', buffering=0)
        # Unlink before saturation: FUSE may need metadata space to create its
        # hidden backing entry. Keep writing through the already unlinked handle.
        ballast.write(b'\0' * 4096)
        os.unlink('/quota/lagging-ballast')
        assert os.fstat(ballast.fileno()).st_nlink == 0
        report['filesystem_after_initial_unlink'] = physical()
        written = 4096
        started = time.monotonic()
        # A failed large allocation can leave a smaller extent available. Exhaust
        # the tail at the actual filesystem block size before asserting full.
        for chunk_size in (4 * 1024**2, os.statvfs('/quota').f_frsize):
            block = b'\0' * chunk_size
            while True:
                try:
                    written += ballast.write(block)
                except OSError as error:
                    assert error.errno == errno.ENOSPC, error
                    break
        assert os.fstat(ballast.fileno()).st_nlink == 0
        full = physical()
        report['filesystem_with_full_unlinked_ballast'] = full
        assert full['image_bytes'] == CAPACITY and full['image_allocated_bytes'] <= CAPACITY
        try:
            with open('/quota/extra-byte', 'wb', buffering=0) as extra:
                extra.write(b'x' * 4096)
        except OSError as error:
            assert error.errno == errno.ENOSPC
        else:
            raise AssertionError('hard filesystem capacity did not include held-unlinked ballast')
        note('fixed_image_bounds_metadata_and_open_unlinked_bytes', physical_full=full,
             filled_bytes=written, fill_seconds=time.monotonic()-started,
             held_unlinked_ballast_bytes=os.fstat(ballast.fileno()).st_blocks*512)
        before = request('stats')['stats']
        response = request('publish', count=100)
        assert int(response['elapsed_ns']) < 1_000_000_000, response
        failed = until('writer exposes ENOSPC loss', lambda: (
            lambda stats: stats if stats['io_errors'] > before['io_errors'] and stats['dropped_records'] > before['dropped_records'] else None
        )(request('stats')['stats']))
        assert failed['dropped_sequence_min'] is not None and failed['dropped_sequence_max'] is not None
        note('native_publish_remains_nonblocking_and_loss_is_explicit', full_publish=response, failure_stats=failed)
        ballast.close()
        ballast = None
        until('closing held ballast releases filesystem blocks', lambda: physical()['available_bytes'] > 1024**3)
        # Releasing our descriptor alone must not release Alloy's stalled reader.
        lagger.close()
        lagger = None
        assert inode_fds(alloy.pid, first_inode)
        os.kill(alloy.pid, signal.SIGCONT)
        cleanup_started = time.monotonic()
        until('Alloy drops deleted lifecycle descriptor after discovery resumes', lambda: not inode_fds(alloy.pid, first_inode), seconds=30)
        note('alloy_file_discovery_closes_retained_deleted_inode', alloy_cleanup_seconds=time.monotonic()-cleanup_started)
        recovery_deadline = time.monotonic() + 15
        while time.monotonic() < recovery_deadline:
            request('publish', count=1)
            time.sleep(.25)
            current = request('stats')['stats']
            if current['written_records'] > failed['written_records']:
                break
        else:
            raise AssertionError('native writer did not recover after filesystem capacity release')
        read = request('read')
        assert any(control['io_errors'] > 0 and control['dropped_records'] > 0 for control in read['controls']), read
        note('new_segment_control_retains_cumulative_disk_loss_after_recovery', recovery_reader=read, recovery_stats=current)
        shutdown = request('shutdown')
        assert shutdown['drained']
        fixture.stdin.close()
        assert fixture.wait(timeout=10) == 0
        fixture = None
        assert physical()['image_bytes'] == CAPACITY
        report['filesystem_final'] = physical()
        events = pathlib.Path('/sys/fs/cgroup/memory.events').read_text()
        report['memory_events'] = events
        assert all(int(line.split()[1]) == 0 for line in events.splitlines() if line.split()[0] in ('oom', 'oom_kill', 'oom_group_kill')), events
        report['memory_peak_bytes'] = int(pathlib.Path('/sys/fs/cgroup/memory.peak').read_text())
        report['status'] = 'passed'
    except BaseException as error:
        report.update(status='failed', error=repr(error))
        raise
    finally:
        if ballast is not None:
            ballast.close()
        if lagger is not None:
            lagger.close()
        if fixture is not None:
            fixture.kill(); fixture.wait(timeout=10)
        if alloy is not None:
            os.kill(alloy.pid, signal.SIGCONT)
            alloy.terminate()
            try:
                alloy.wait(timeout=10)
            except subprocess.TimeoutExpired:
                alloy.kill(); alloy.wait(timeout=10)
        if mounted:
            subprocess.run(['umount', '/quota'], timeout=15, check=False)
        pathlib.Path('/report.json').write_text(json.dumps(report, indent=2)+'\n')


NATIVE_TEST = 'dynamic_enospc_bidirectional_cross_chain_smoke'


def native_inside():
    """Real native protocol work starts only after the writer hits a full mount."""
    control = pathlib.Path('/control')
    control.mkdir()
    logs = pathlib.Path('/reports/logs')
    logs.mkdir(parents=True)
    report = {'status': 'failed', 'checks': [], 'image_capacity_bytes': CAPACITY,
              'scope': 'actual native bidirectional protocol and writer under physical ENOSPC; no overhead claim'}
    process = alloy = ballast = lagger = None
    mounted = False

    def physical():
        value = os.statvfs('/quota')
        return {'capacity_bytes': value.f_blocks*value.f_frsize,
                'available_bytes': value.f_bavail*value.f_frsize,
                'image_bytes': os.stat('/volume.ext4').st_size}

    def marker(name):
        path = control/(name+'.json')
        if path.exists():
            return json.loads(path.read_text())
        if process.poll() is not None:
            raise AssertionError(f'native test exited before {name}: {process.returncode}')
        return None

    def signal_marker(name, value):
        temporary = control/(name+'.partial')
        temporary.write_text(json.dumps(value))
        temporary.replace(control/(name+'.json'))

    try:
        backing = os.statvfs('/')
        assert backing.f_bavail*backing.f_frsize > CAPACITY*2, 'insufficient disposable backing storage'
        with open('/volume.ext4', 'wb') as image:
            image.truncate(CAPACITY)
        run(['mkfs.ext4','-q','-F','-m','0','-b','4096','-N','65536','/volume.ext4'])
        pathlib.Path('/quota').mkdir()
        run(['fuse2fs','/volume.ext4','/quota'])
        mounted = True
        assert physical()['capacity_bytes'] <= CAPACITY
        listing = run(['/fixture/native-test','--list','--ignored','--exact',NATIVE_TEST])
        assert NATIVE_TEST+': test' in listing, 'required ignored native fixture is missing from archived binary'
        env = dict(os.environ, CARGO_MANIFEST_DIR='/repo/crates/eez-testkit',
                   EEZ_TEST_PREBUILT='1', EEZ_TEST_RUNTIME_BIN_DIR='/fixture/runtime',
                   EEZ_TEST_PROOF_SIGNER_BIN='/fixture/runtime/eez-proof-signer',
                   EEZ_TEST_LOG_DIR='/reports/logs', EEZ_TEST_DATADIR_DIR='/reports/datadirs',
                   EEZ_TEST_ENOSPC_CONTROL=str(control), EEZ_TEST_ENOSPC_SPOOL='/quota/spool')
        process = subprocess.Popen(['/fixture/native-test',NATIVE_TEST,'--ignored','--exact','--test-threads=1'],
            cwd='/repo',env=env,stdout=open('/reports/native-test.log','w'),stderr=subprocess.STDOUT,
            start_new_session=True)
        healthy = until('initial healthy native writer', lambda: marker('healthy'), seconds=180)
        assert healthy['written_records'] > 0 and healthy['io_errors'] == 0 and healthy['dropped_records'] == 0
        report['initial_stats'] = healthy
        report['checks'].append('native_writer_started_and_persisted_real_records_before_failure')
        first = next(pathlib.Path('/quota/spool').glob('*.active'))
        lagger = open(first,'rb')
        inode = (os.fstat(lagger.fileno()).st_dev, os.fstat(lagger.fileno()).st_ino)
        run(['alloy','validate','/fixture/alloy.alloy'])
        alloy = subprocess.Popen(['alloy','run','--disable-reporting','--storage.path=/alloy-data',
            '--server.http.listen-addr=127.0.0.1:12345','/fixture/alloy.alloy'],
            stdout=open('/reports/alloy.log','w'),stderr=subprocess.STDOUT)
        until('Alloy opened actual native spool',lambda: inode_fds(alloy.pid,inode))
        os.kill(alloy.pid,signal.SIGSTOP)
        ballast = open('/quota/held-ballast','wb',buffering=0)
        ballast.write(b'\0'*4096)
        os.unlink('/quota/held-ballast')
        written = 4096
        for size in (4*1024**2,os.statvfs('/quota').f_frsize):
            block = b'\0'*size
            while True:
                try:
                    written += ballast.write(block)
                except OSError as error:
                    assert error.errno == errno.ENOSPC, error
                    break
        assert os.fstat(ballast.fileno()).st_nlink == 0
        full = physical()
        assert full['image_bytes'] == CAPACITY
        assert inode_fds(alloy.pid,inode), 'stopped reader lost native descriptor'
        report.update(full_filesystem=full,held_unlinked_bytes=os.fstat(ballast.fileno()).st_blocks*512,filled_bytes=written)
        signal_marker('full',{'enospc': True, 'held_unlinked': True})
        completed = until('native semantics complete during disk failure',lambda: marker('completed_full'),seconds=240)
        assert completed['writer_errors'] > 0 and completed['dropped_io'] > 0
        assert completed['source_count'] == 2 and inode_fds(alloy.pid,inode)
        report['full_native_result'] = completed
        report['checks'].append('real_writer_io_loss_and_bidirectional_effects_canonical_state_and_signature_while_full')
        report['checks'].append('stopped_alloy_and_held_unlinked_ballast_remain_present_during_protocol_progress')
        ballast.close(); ballast = None
        until('released ballast restores space',lambda: physical()['available_bytes'] > 1024**3)
        lagger.close(); lagger = None
        os.kill(alloy.pid,signal.SIGCONT)
        signal_marker('released',{'available_bytes':physical()['available_bytes'],
                                  'observed_at_unix_ms':int(time.time()*1000)})
        recovered = until('native writer resumes with cumulative loss control',lambda: marker('recovered'),seconds=180)
        assert recovered['fresh_post_release_persisted_commits'] == 2
        assert recovered['io_errors'] > 0 and recovered['dropped_records'] > 0
        assert recovered['dropped_sequence_min'] is not None and recovered['dropped_sequence_max'] is not None
        assert recovered['written_records'] > healthy['written_records']
        report['recovered_stats'] = recovered
        assert process.wait(timeout=60) == 0
        result = pathlib.Path('/reports/native-test.log').read_text()
        assert '1 passed; 0 failed' in result, 'required native test did not execute exactly once'
        report['checks'].append('native_semantics_test_passes_and_writer_recovers_without_restart')
        report['memory_events'] = pathlib.Path('/sys/fs/cgroup/memory.events').read_text()
        assert all(int(line.split()[1])==0 for line in report['memory_events'].splitlines()
                   if line.split()[0] in ('oom','oom_kill','oom_group_kill'))
        report['memory_peak_bytes'] = int(pathlib.Path('/sys/fs/cgroup/memory.peak').read_text())
        report['status'] = 'passed'
    finally:
        if process is not None and process.poll() is None:
            os.killpg(process.pid,signal.SIGKILL); process.wait(timeout=10)
        if alloy is not None:
            os.kill(alloy.pid,signal.SIGCONT); alloy.terminate()
            try: alloy.wait(timeout=10)
            except subprocess.TimeoutExpired: alloy.kill(); alloy.wait(timeout=10)
        for handle in (ballast,lagger):
            if handle is not None: handle.close()
        if mounted: subprocess.run(['umount','/quota'],timeout=15,check=False)
        pathlib.Path('/reports/report.json').write_text(json.dumps(report,indent=2)+'\n')


def native_outer(args):
    if args.runtime_bin is None:
        raise ValueError('--native-test requires --runtime-bin; no runtime build fallback exists')
    binary = args.native_test.resolve(strict=True)
    runtime = args.runtime_bin.resolve(strict=True)
    for name in ('eez-composer','eez-follower','eez-proof-signer','eez-observer'):
        if not (runtime/name).is_file():
            raise ValueError('required supplied runtime artifact missing: '+name)
    anvil = pathlib.Path(shutil.which('anvil') or '').resolve(strict=True)
    if not anvil.is_file(): raise ValueError('pinned Foundry anvil is required')
    anvil_version = run([str(anvil),'--version'])
    if not re.search(r'Version: 1\.7\.1(?:\s|$)', anvil_version):
        raise ValueError('native quota fixture requires Foundry anvil1.7.1')
    container = 'eez-native-quota-'+uuid.uuid4().hex[:12]
    image_id = json.loads(run(['docker','image','inspect',args.image]))[0]['Id']
    args.report.parent.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='eez-native-quota-artifacts-') as directory:
        snapshot = pathlib.Path(directory)
        # Container root has no DAC_OVERRIDE; this read-only fixture contains
        # public binaries/scripts, so allow traversal of its owned snapshot.
        snapshot.chmod(0o755)
        shutil.copy2(binary,snapshot/'native-test')
        (snapshot/'runtime').mkdir()
        for name in ('eez-composer','eez-follower','eez-proof-signer','eez-observer'):
            shutil.copy2(runtime/name,snapshot/'runtime'/name)
        shutil.copy2(anvil,snapshot/'anvil')
        shutil.copy2(pathlib.Path(__file__),snapshot/'drill.py')
        shutil.copy2(ROOT/'tests/spool-quota/alloy.alloy',snapshot/'alloy.alloy')
        # Only public runtime-read fixture inputs are mounted, and their owned
        # snapshots cannot change when another lane edits the shared checkout.
        repo = snapshot/'repo'
        (repo/'crates/eez-testkit').mkdir(parents=True)
        inputs = [ROOT.parent/'scripts/builder-stub.py',
                  ROOT.parent/'crates/eez-node-e2e/tests/fixtures/genesis.json',
                  *sorted((ROOT.parent/'contracts/out').rglob('*.json'))]
        if len(inputs)<=2:
            raise ValueError('compiled pinned contract artifacts are required')
        def file_hash(path):
            with path.open('rb') as stream:
                return hashlib.file_digest(stream,'sha256').hexdigest()
        repo_hashes = {}
        for source in inputs:
            relative = source.relative_to(ROOT.parent)
            before = file_hash(source)
            destination = repo/relative
            destination.parent.mkdir(parents=True,exist_ok=True)
            shutil.copy2(source,destination)
            if file_hash(destination)!=before:
                raise ValueError('fixture input changed while snapshotting: '+str(relative))
            repo_hashes[str(relative)] = before

        report = {'status':'failed','container':container,'image_id':image_id,
                  'anvil_version':anvil_version,'repo_input_sha256':repo_hashes,
                  'artifact_sha256':{str(p.relative_to(snapshot)):file_hash(p)
                                     for p in snapshot.rglob('*') if p.is_file() and repo not in p.parents}}
        try:
            command = ['docker','run','--name',container,'--network','none','--memory','4g','--memory-swap','4g',
                '--cpus','4','--pids-limit','512','--cap-drop','ALL','--cap-add','SYS_ADMIN','--device','/dev/fuse',
                '--security-opt','apparmor=unconfined','--security-opt','no-new-privileges:true',
                '--mount',f'type=bind,src={repo},dst=/repo,readonly',
                '--mount',f'type=bind,src={snapshot},dst=/fixture,readonly',
                '--mount',f'type=bind,src={snapshot}/anvil,dst=/usr/local/bin/anvil,readonly',
                image_id,'python3','/fixture/drill.py','--inside','--native-mode']
            result = subprocess.run(command,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=900)
            print(result.stdout,end='',flush=True)
            run(['docker','cp',container+':/reports/report.json',str(args.report)])
            report.update(json.loads(args.report.read_text()))
            state = json.loads(run(['docker','inspect',container]))[0]['State']
            report['container_state'] = {key:state[key] for key in ('OOMKilled','ExitCode','Error')}
            assert result.returncode==0 and not state['OOMKilled'] and report['status']=='passed',report
            after = {name:file_hash(repo/name) for name in repo_hashes}
            assert after==repo_hashes, 'runtime-read fixture snapshot changed'
            report['repo_inputs_unchanged_after_run'] = True
        finally:
            destination = args.report.with_name(args.report.stem+'-native')
            subprocess.run(['docker','cp',container+':/reports',str(destination)],stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL,timeout=60,check=False)
            subprocess.run(['docker','rm','--force','--volumes',container],stdout=subprocess.DEVNULL,timeout=30,check=False)
            args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--inside', action='store_true')
    parser.add_argument('--native-mode', action='store_true', help=argparse.SUPPRESS)
    parser.add_argument('--native-test', type=pathlib.Path)
    parser.add_argument('--runtime-bin', type=pathlib.Path)
    parser.add_argument('--binary', type=pathlib.Path, default=ROOT.parent/'target/debug/examples/spool-fixture')
    parser.add_argument('--image', default='eez-spool-quota-fixture:local')
    parser.add_argument('--report', type=pathlib.Path)
    args = parser.parse_args()
    if args.native_mode and not args.inside:
        parser.error('--native-mode is an inside-only runner option')
    if args.inside:
        (native_inside if args.native_mode else inside)(); return
    if args.report is None:
        parser.error('--report is required')
    if args.native_test is not None:
        native_outer(args); return
    if args.runtime_bin is not None:
        parser.error('--runtime-bin requires --native-test')
    binary = args.binary.resolve(strict=True)
    if not binary.is_file():
        raise ValueError('fixture binary must exist; build the telemetry example first')
    # Check capacity before starting; no shared host filesystem is filled.
    memory = {key: int(value.split()[0])*1024 for key, value in (line.split(':',1) for line in pathlib.Path('/proc/meminfo').read_text().splitlines())}
    if memory['MemAvailable'] < 4*1024**3:
        raise RuntimeError('at least 4GiB available host memory required for isolated 1GiB test budget')
    container = 'eez-spool-quota-' + uuid.uuid4().hex[:12]
    image_id = json.loads(run(['docker','image','inspect',args.image]))[0]['Id']
    args.report.parent.mkdir(parents=True,exist_ok=True)
    snapshots = tempfile.TemporaryDirectory(prefix='eez-spool-artifacts-')
    snapshot = pathlib.Path(snapshots.name)
    shutil.copy2(binary, snapshot/'spool-fixture')
    shutil.copy2(pathlib.Path(__file__), snapshot/'drill.py')
    shutil.copy2(ROOT/'tests/spool-quota/alloy.alloy', snapshot/'alloy.alloy')
    binary = snapshot/'spool-fixture'
    report = {'status':'failed','container':container,'image_id':image_id,'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(), 'host_available_memory_bytes':memory['MemAvailable']}
    try:
        command = ['docker','run','--name',container,'--network','none','--memory','1g','--memory-swap','1g',
            '--cpus','2','--pids-limit','128','--cap-drop','ALL','--cap-add','SYS_ADMIN','--device','/dev/fuse',
            '--security-opt','apparmor=unconfined','--security-opt','no-new-privileges:true',
            '--log-driver','local','--log-opt','max-size=1m','--log-opt','max-file=2',
            '--mount',f'type=bind,src={binary},dst=/fixture/spool-fixture,readonly',
            '--mount',f'type=bind,src={snapshot}/drill.py,dst=/fixture/drill.py,readonly',
            '--mount',f'type=bind,src={snapshot}/alloy.alloy,dst=/fixture/alloy.alloy,readonly',
            args.image,'python3','/fixture/drill.py','--inside']
        result = subprocess.run(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=360)
        print(result.stdout, end='', flush=True)
        run(['docker','cp',container+':/report.json',str(args.report)])
        report.update(json.loads(args.report.read_text()))
        state = json.loads(run(['docker','inspect',container]))[0]['State']
        report['container_state'] = {key: state[key] for key in ('OOMKilled','ExitCode','Error')}
        assert result.returncode == 0 and not state['OOMKilled'], report
        assert report['binary_sha256'] == report['mounted_binary_sha256'], 'executed binary differed from immutable snapshot'
    except BaseException as error:
        report.update(status='failed', error=repr(error))
        raise
    finally:
        for source, suffix in [('/alloy.log','alloy'),('/fixture-stderr.log','native')]:
            subprocess.run(['docker','cp',container+':'+source,str(args.report.with_name(args.report.stem+'-'+suffix+'.log'))],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=30,check=False)
        subprocess.run(['docker','rm','--force','--volumes',container],stdout=subprocess.DEVNULL,timeout=30,check=False)
        args.report.write_text(json.dumps(report,indent=2)+'\n')
        snapshots.cleanup()
    print(json.dumps(report,indent=2))


if __name__ == '__main__':
    main()
