#!/usr/bin/env python3
"""Pinned Lighthouse endpoints/logs in network-isolated genesis-only fixtures."""
import argparse
import json
import pathlib
import subprocess
import time
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
IMAGES = json.loads((ROOT/'tests/client-images.lock.json').read_text())
PYTHON_IMAGES = json.loads((ROOT/'tests/images.lock.json').read_text())
PYTHON = next(name+'@'+digest for name,digest in PYTHON_IMAGES.items() if name.startswith('python:'))


def run(command, check=True):
    value = subprocess.run(command,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=90)
    if check and value.returncode:
        raise RuntimeError(value.stdout[-3000:])
    return value


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--report',type=pathlib.Path,required=True)
    args=parser.parse_args()
    args.report.parent.mkdir(parents=True,exist_ok=True)
    report={'status':'failed','scope':'pinned binary endpoint/file capabilities only; isolated bundled mainnet genesis, no live chain or execution endpoint, no sync/finality acceptance','clients':[]}
    owned=[]
    try:
        for tag,digest in IMAGES.items():
            image=tag+'@'+digest
            name='eez-client-capability-'+uuid.uuid4().hex[:12]
            entry={'image':image,'checks':[]}
            report['clients'].append(entry)
            help_text=run(['docker','run','--rm','--network','none','--cap-drop','ALL',image,'lighthouse','beacon_node','--help']).stdout
            for flag in ['--metrics','--metrics-address','--metrics-port','--logfile-dir','--logfile-format','--logfile-max-size','--logfile-max-number','--logfile-no-restricted-perms']:
                assert flag in help_text,flag
            entry['checks'].append('pinned_cli_metrics_and_structured_file_capabilities')
            command=['docker','run','-d','--name',name,'--network','none','--memory','2g','--memory-swap','2g','--cpus','2',
                '--user','10001:10001','--cap-drop','ALL','--security-opt','no-new-privileges:true','--log-driver','local','--log-opt','max-size=2m','--log-opt','max-file=2',
                '--tmpfs','/data:rw,mode=0700,uid=10001,gid=10001,size=536870912','--tmpfs','/logs:rw,mode=0750,uid=10001,gid=10001,size=33554432',
                '--entrypoint','sh',image,'-c','umask 027\nexec lighthouse "$@"','fixture','beacon_node',
                '--network','mainnet','--allow-insecure-genesis-sync','--datadir','/data','--listen-address','127.0.0.1',
                '--disable-upnp','--target-peers','0','--execution-endpoint','http://127.0.0.1:9',
                '--execution-jwt-secret-key','00'*32,'--metrics','--metrics-address','127.0.0.1','--metrics-port','5054',
                '--logfile-dir','/logs','--logfile-format','JSON','--logfile-debug-level','info','--logfile-max-size','10',
                '--logfile-max-number','3']
            run(command);owned.append(name)
            deadline=time.monotonic()+45
            while time.monotonic()<deadline:
                logs=run(['docker','logs',name],check=False).stdout
                if 'Metrics HTTP server started' in logs:
                    break
                state=json.loads(run(['docker','inspect',name]).stdout)[0]['State']
                assert state['Running'], 'isolated Lighthouse exited before metrics initialization'
                time.sleep(.2)
            else:
                raise AssertionError('metrics listener did not initialize')
            code='import sys,json,urllib.request;sys.path.insert(0,"/fixture");from client_metrics import inventory;data=urllib.request.urlopen("http://127.0.0.1:5054/metrics",timeout=5).read(16777217);assert len(data)<=16777216;print(json.dumps(inventory(data.decode())))'
            raw=run(['docker','run','--rm','--network','container:'+name,'--cap-drop','ALL','--security-opt','no-new-privileges:true',
                '--mount',f'type=bind,src={ROOT}/tests/client_metrics.py,dst=/fixture/client_metrics.py,readonly',PYTHON,'python3','-c',code]).stdout
            metrics=json.loads(raw)
            expected={'lighthouse_info','beacon_head_slot','beacon_finalized_epoch','sync_eth2_synced','process_resident_memory_bytes'}
            deadline=time.monotonic()+30
            while not expected <= set(metrics['samples']) and time.monotonic()<deadline:
                time.sleep(1)
                raw=run(['docker','run','--rm','--network','container:'+name,'--cap-drop','ALL','--security-opt','no-new-privileges:true',
                    '--mount',f'type=bind,src={ROOT}/tests/client_metrics.py,dst=/fixture/client_metrics.py,readonly',PYTHON,'python3','-c',code]).stdout
                metrics=json.loads(raw)
            assert expected <= set(metrics['samples']), expected-set(metrics['samples'])
            entry['inventory']=metrics
            entry['checks'].append('actual_private_metrics_family_type_and_label_key_sample')
            content=run(['docker','exec',name,'cat','/logs/beacon.log']).stdout
            records=[json.loads(line) for line in content.splitlines() if line]
            assert records and all(isinstance(record,dict) and {'level','msg','ts'}<=set(record) for record in records)
            mode=run(['docker','exec',name,'stat','-c','%a','/logs/beacon.log']).stdout.strip()
            assert mode=='600', f'default log must remain private to its explicit owner: {mode}'
            entry['beacon_log']={'mode':mode,'lines':len(records),'field_keys':sorted({key for record in records for key in record})}
            entry['checks'].append('actual_json_beacon_log_nonroot_owner_only_no_public_read')
            state=json.loads(run(['docker','inspect',name]).stdout)[0]['State']
            assert not state['OOMKilled']
            run(['docker','rm','--force','--volumes',name]);owned.remove(name)
        report['status']='passed'
    except BaseException as error:
        report['error']=repr(error)
        raise
    finally:
        for name in owned:
            diagnostic=run(['docker','logs',name],check=False).stdout
            args.report.with_name(args.report.stem+'-'+name+'.log').write_text(diagnostic)
            run(['docker','rm','--force','--volumes',name],check=False)
        diagnostics = [path.read_text() for path in args.report.parent.glob(args.report.stem+'-*.log')]
        args.report.with_suffix('.logs.txt').write_text('\n'.join(diagnostics))
        args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({'status':report['status'],'clients':[{'image':entry['image'],'families':len(entry['inventory']['types']),'sample_names':len(entry['inventory']['samples']),'checks':entry['checks'],'log_mode':entry['beacon_log']['mode']} for entry in report['clients']]},indent=2))


if __name__=='__main__':main()
