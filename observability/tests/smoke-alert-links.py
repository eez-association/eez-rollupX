#!/usr/bin/env python3
"""Delivered Grafana URLs must resolve to private runbooks and matching dashboards."""
import argparse
import hashlib
import importlib.util
import json
import pathlib
import socket
import subprocess
import tempfile
import urllib.error
import urllib.parse
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('alert_link_helpers', ROOT/'tests/smoke-traces.py')
h = importlib.util.module_from_spec(spec)
spec.loader.exec_module(h)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--report', type=pathlib.Path, required=True)
    args = parser.parse_args()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    project = 'eez-alert-links-' + uuid.uuid4().hex[:10]
    images = json.loads((ROOT/'tests/images.lock.json').read_text())
    fixture_image = next(k+'@'+v for k,v in images.items())
    report = {'status':'failed','checks':[], 'images':{**json.loads((ROOT/'images.lock.json').read_text()), **images}, 'scope':'actual pinned Grafana notifications and private browser navigation; synthetic alert inputs'}
    report['source_sha256'] = {str(path.relative_to(ROOT)):hashlib.sha256(path.read_bytes()).hexdigest()
        for path in [ROOT/'prepare.py',ROOT/'compose.yaml',ROOT/'tests/smoke-alert-links.py',ROOT/'tests/verify-alert-links.cjs',
                     ROOT/'runbooks/operations.md',ROOT/'runbooks/protocol-alerts.md',ROOT/'images.lock.json',ROOT/'tests/images.lock.json',
                     ROOT/'tests/fixture-server.py',ROOT/'tests/smoke-traces.py']}
    with tempfile.TemporaryDirectory(prefix=project+'-') as temporary:
        directory = pathlib.Path(temporary)
        (directory/'logs').mkdir()
        lease = socket.socket()
        lease.bind(('127.0.0.1',0))
        port = lease.getsockname()[1]
        origin = f'http://127.0.0.1:{port}'
        config = {'environment':'devnet','deployment':project,'observer_instance':'fixture','observer_target':'fixture:9000',
                  'log_directory':str(directory/'logs'),'alert_webhook_url':'http://fixture:9000/alerts',
                  'heartbeat_webhook_url':'http://fixture:9000/heartbeat','grafana_port':port,'grafana_public_url':origin,
                  'prometheus_port':0,'loki_port':0,'database_enabled':True,'protocol_alerts':{'owner':'fixture-operator'}}
        runtime = h.prepare.prepare(config,directory/'runtime',fast_test=True)
        override = directory/'fixture.json'
        override.write_text(json.dumps({'services':{'fixture':{'image':fixture_image,'command':['python3','/fixture.py'],
            'ports':['127.0.0.1:0:9000'],'volumes':[str(ROOT/'tests/fixture-server.py')+':/fixture.py:ro'],
            'mem_limit':'128m','cap_drop':['ALL'],'security_opt':['no-new-privileges:true']}}}))
        compose = ['docker','compose','--project-name',project,'--env-file',str(runtime/'compose.env'),
                   '-f',str(ROOT/'compose.yaml'),'-f',str(override)]
        try:
            lease.close()
            h.run(compose+['up','-d','--wait','--wait-timeout','150','fixture','prometheus','grafana'])
            fixture = 'http://'+h.run(compose+['port','fixture','9000']).strip()
            assert 'http://'+h.run(compose+['port','grafana','3000']).strip() == origin
            password = (runtime/'secrets/grafana-admin-password').read_text()
            try:
                h.request(origin+'/api/dashboards/uid/eez-runbooks')
            except urllib.error.HTTPError as error:
                assert error.code == 401
            else:
                raise AssertionError('runbook API became public')
            report['checks'].append('runbooks_remain_private')
            def captured():
                selected = {}
                for event in h.request(fixture+'/events'):
                    for alert in event['data'].get('alerts',[]):
                        url = alert.get('annotations',{}).get('runbook_url','')
                        panel = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query).get('viewPanel',[''])[0]
                        if panel in ('1','2'):
                            selected.setdefault(panel,alert)
                return [selected['1'],selected['2']] if len(selected)==2 else None
            alerts = h.until('actual operations and protocol runbook notifications',captured,seconds=100)
            for alert in alerts:
                for key in ('generatorURL','dashboardURL','panelURL'):
                    url = urllib.parse.urlsplit(alert[key])
                    assert url.scheme+'://'+url.netloc == origin, alert
                assert alert['annotations']['runbook_url'].startswith(origin+'/d/eez-runbooks?viewPanel=')
                uid = urllib.parse.urlsplit(alert['dashboardURL']).path.split('/')[2]
                dashboard = h.request(origin+'/api/dashboards/uid/'+uid,password=password)
                assert dashboard['dashboard']['uid']==uid
            report['checks'].append('delivered_generator_dashboard_panel_and_runbook_urls_use_configured_origin')
            browser_output = args.report.with_name(args.report.stem+'-browser').resolve()
            browser_input = directory/'browser.json'
            browser_input.write_text(json.dumps({'url':origin,'password':password,'alerts':alerts,'output':str(browser_output)}))
            browser_input.chmod(0o600)
            subprocess.run(['node',str(ROOT/'tests/verify-alert-links.cjs'),str(browser_input)],check=True,timeout=120)
            report['browser'] = json.loads((browser_output/'browser.json').read_text())
            assert report['browser']['status']=='passed'
            report['checks'].append('actual_browser_follows_delivered_links_and_renders_both_runbooks')
            report['alerts'] = [{'alertname':a['labels']['alertname'],'dashboard_path':urllib.parse.urlsplit(a['dashboardURL']).path,
                                  'runbook_path':urllib.parse.urlsplit(a['annotations']['runbook_url']).path,
                                  'runbook_panel':urllib.parse.parse_qs(urllib.parse.urlsplit(a['annotations']['runbook_url']).query)['viewPanel'][0]} for a in alerts]
            assert report['source_sha256'] == {name:hashlib.sha256((ROOT/name).read_bytes()).hexdigest()
                for name in report['source_sha256']}, 'fixture sources changed during execution'
            report['status']='passed'
        finally:
            lease.close()
            args.report.with_suffix('.logs.txt').write_text(h.run(compose+['logs','--no-color'],check=False))
            h.run(compose+['down','--volumes','--remove-orphans'],check=False)
            remaining = [h.run(command+['--filter','label=com.docker.compose.project='+project]).strip()
                for command in [['docker','ps','-aq'],['docker','volume','ls','-q']]]
            report['owned_resources_removed'] = not any(remaining)
            if any(remaining):
                report['status']='cleanup_failed'
            args.report.write_text(json.dumps(report,indent=2)+'\n')
            assert not any(remaining), 'owned disposable resources remain'
    print(json.dumps(report,indent=2))


if __name__=='__main__':
    main()
