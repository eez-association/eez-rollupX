"""Real FK inserts must coexist with chain writers, while writers stay serialized."""
import os
import pathlib
import re
import select
import subprocess
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]


class Session:
    def __init__(self, psql):
        self.process = subprocess.Popen(psql + ['-Atq'], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        self.buffer = b''

    def send(self, sql, marker):
        self.process.stdin.write((sql + '\n\\echo ' + marker + '\n').encode())
        self.process.stdin.flush()

    def ready(self, marker):
        deadline = time.monotonic() + 10
        token = (marker + '\n').encode()
        while token not in self.buffer:
            remaining = deadline - time.monotonic()
            assert remaining > 0, self.buffer.decode(errors='replace')
            if select.select([self.process.stdout], [], [], remaining)[0]:
                chunk = os.read(self.process.stdout.fileno(), 4096)
                assert chunk, self.buffer.decode(errors='replace')
                self.buffer += chunk
        self.buffer = self.buffer.split(token, 1)[1]

    def close(self):
        if self.process.poll() is None:
            try:
                self.process.stdin.write(b'ROLLBACK;\n\\q\n')
                self.process.stdin.flush()
                self.process.wait(timeout=5)
            except (BrokenPipeError, subprocess.TimeoutExpired):
                self.process.kill()
                self.process.wait(timeout=5)
        self.process.stdin.close()
        self.process.stdout.close()


def exercise(run, psql, scalar, report):
    source = (ROOT / 'crates/eez-observer/src/indexer/ledger.rs').read_text()
    # Execute the actual chain lock statement; the caller intentionally assigns
    # L2 the lower database ID, as concurrent native startup can do.
    match = re.search(r'"(SELECT frozen_reason FROM eez.chain_instances WHERE id=\$1 FOR [^"]+)"', source)
    assert match, 'production chain lock query not found'
    lock = match.group(1)
    deployment = int(scalar("INSERT INTO eez.deployments(environment,name) VALUES('fixture','chain-locks') RETURNING id;"))
    chains = []
    lifecycle = None
    holder = other = None
    try:
        for role, chain_id in [('l2', 2), ('l1', 1)]:
            rollup = 0 if role == 'l1' else 1
            chains.append(int(scalar(f"INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number) VALUES({deployment},'{role}',{chain_id},decode(repeat('19',32),'hex'),{rollup},0) RETURNING id;")))
        low, high = chains
        lifecycle = int(scalar(f"INSERT INTO eez.lifecycle_sources(deployment_id,service,instance,directory) VALUES({deployment},'composer','chain-locks','/fixture') RETURNING id;"))
        epoch = "decode(repeat('19',16),'hex')"
        run(psql, data=f"INSERT INTO eez.lifecycle_epochs(source_id,epoch) VALUES({lifecycle},{epoch}); INSERT INTO eez.lifecycle_events(source_id,epoch,sequence,chunk_count,state) VALUES({lifecycle},{epoch},1,1,'incomplete');")
        insert = f"INSERT INTO eez.lifecycle_cost_checks(source_id,epoch,sequence,context_l1,context_l2,state,record_refs) VALUES({lifecycle},{epoch},1,{high},{low},'missing_record','[]');"
        holder = Session(psql)
        holder.send(f"BEGIN; SELECT id FROM eez.chain_instances WHERE id={low} FOR UPDATE;", 'old_lock')
        holder.ready('old_lock')
        blocked = run(psql, data="SET lock_timeout='200ms'; " + insert, check=False)
        assert blocked.returncode != 0 and 'lock timeout' in blocked.stdout, blocked.stdout
        holder.send('ROLLBACK;', 'old_released')
        holder.ready('old_released')
        holder.send("BEGIN; SET application_name='eez-chain-lock-holder'; " + lock.replace('$1', str(low)) + ';', 'new_lock')
        holder.ready('new_lock')
        # The holder has not committed: passing cannot be explained by a race
        # where the writer released its lock before the real FK checks ran.
        run(psql, data="SET lock_timeout='1s'; " + insert)
        assert scalar(f'SELECT count(*) FROM eez.lifecycle_cost_checks WHERE source_id={lifecycle};') == '1'
        other = Session(psql)
        other.send("BEGIN; SET application_name='eez-chain-lock-waiter'; " + lock.replace('$1', str(low)) + ';', 'second_lock')
        deadline = time.monotonic() + 5
        while scalar("SELECT EXISTS(SELECT FROM pg_stat_activity a WHERE a.application_name='eez-chain-lock-waiter' AND cardinality(pg_blocking_pids(a.pid))>0);") != 't':
            assert time.monotonic() < deadline, 'second canonical writer was not serialized'
            time.sleep(.02)
        holder.send('COMMIT;', 'released')
        holder.ready('released')
        other.ready('second_lock')
        other.send('COMMIT;', 'done')
        other.ready('done')
        report['checks'].append('chain_writer_lock_allows_reversed_context_foreign_keys_but_serializes_other_writers')
    finally:
        for session in (other, holder):
            if session is not None:
                session.close()
        if lifecycle is not None:
            run(psql, data=f'DELETE FROM eez.lifecycle_cost_checks WHERE source_id={lifecycle}; DELETE FROM eez.lifecycle_events WHERE source_id={lifecycle}; DELETE FROM eez.lifecycle_epochs WHERE source_id={lifecycle}; DELETE FROM eez.lifecycle_sources WHERE id={lifecycle};')
        if chains:
            run(psql, data='DELETE FROM eez.chain_instances WHERE id IN (' + ','.join(map(str, chains)) + ');')
        run(psql, data=f'DELETE FROM eez.deployments WHERE id={deployment};')
