"""Canonical revision transaction/concurrency regressions on test-owned rows."""
import subprocess


def exercise(run, psql, scalar, report):
    deployment = int(scalar("INSERT INTO eez.deployments(environment,name) VALUES('fixture','canonical-revision') RETURNING id;"))
    chain = int(scalar(f"INSERT INTO eez.chain_instances(deployment_id,role,chain_id,genesis_hash,rollup_id,bootstrap_number) VALUES({deployment},'l1',1,decode(repeat('15',32),'hex'),0,0) RETURNING id;"))
    block = lambda number: f"decode(lpad(to_hex({number}),64,'0'),'hex')"
    lock = f"SELECT id FROM eez.chain_instances WHERE id={chain} FOR UPDATE;"
    revision = lambda: int(scalar(f'SELECT canonical_revision FROM eez.chain_instances WHERE id={chain};'))
    try:
        run(psql,data=f"BEGIN; {lock} INSERT INTO eez.blocks(chain_instance_id,hash,number,parent_hash,timestamp_seconds,canonical,receipt_coverage) SELECT {chain},decode(lpad(to_hex(n),64,'0'),'hex'),n,decode(repeat('00',32),'hex'),n,true,'complete' FROM generate_series(1,4) n; COMMIT;")
        assert revision() == 0, 'append-only canonical insertion must not invalidate earlier samples'
        run(psql,data=f"BEGIN; {lock} UPDATE eez.blocks SET canonical=true WHERE chain_instance_id={chain}; COMMIT;")
        assert revision() == 0, 'unchanged canonical values do not increment'
        run(psql,data=f"BEGIN; {lock} UPDATE eez.blocks SET canonical=false WHERE chain_instance_id={chain} AND hash={block(1)}; ROLLBACK;")
        assert revision() == 0, 'rolled-back orphaning cannot invalidate committed samples'
        processes=[]
        try:
            for number in (1,2):
                process=subprocess.Popen(psql,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
                process.stdin.write(f"BEGIN; SET LOCAL statement_timeout='10s'; SET LOCAL lock_timeout='5s'; {lock} SELECT pg_sleep(0.2); UPDATE eez.blocks SET canonical=false WHERE chain_instance_id={chain} AND hash={block(number)}; COMMIT;")
                process.stdin.close();process.stdin=None
                processes.append(process)
            for process in processes:
                output=process.communicate(timeout=15)[0]
                assert process.returncode == 0, output
        finally:
            for process in processes:
                if process.poll() is None:
                    process.kill();process.wait(timeout=5)
        assert revision() == 2, 'serialized concurrent writers must not lose increments'
        run(psql,data=f"BEGIN; {lock} DELETE FROM eez.blocks WHERE chain_instance_id={chain} AND hash={block(1)}; COMMIT;")
        assert revision() == 2, 'deleting already orphaned evidence is not another canonical change'
        run(psql,data=f"BEGIN; {lock} DELETE FROM eez.blocks WHERE chain_instance_id={chain} AND hash={block(3)}; COMMIT;")
        assert revision() == 3, 'deleting canonical history invalidates surviving sample references'
        run(psql,data=f"BEGIN; {lock} UPDATE eez.chain_instances SET canonical_revision=9223372036854775807 WHERE id={chain}; COMMIT;")
        overflow=run(psql,data=f"BEGIN; {lock} UPDATE eez.blocks SET canonical=false WHERE chain_instance_id={chain} AND hash={block(4)}; COMMIT;",check=False)
        assert overflow.returncode != 0 and 'out of range' in overflow.stdout
        assert revision() == 9223372036854775807
        assert scalar(f"SELECT canonical FROM eez.blocks WHERE chain_instance_id={chain} AND hash={block(4)};") == 't'
        report['checks'].append('canonical_revision_append_noop_rollback_concurrent_orphan_delete_and_overflow_are_transactional')
    finally:
        # Fixture-only administrative teardown, with the normal lock order. The
        # overflow counter is reset solely so these disposable rows can be removed.
        run(psql,data=f"BEGIN; {lock} UPDATE eez.chain_instances SET canonical_revision=0 WHERE id={chain}; DELETE FROM eez.blocks WHERE chain_instance_id={chain}; DELETE FROM eez.chain_instances WHERE id={chain}; DELETE FROM eez.deployments WHERE id={deployment}; COMMIT;")
