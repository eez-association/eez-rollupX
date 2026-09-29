"""Join retained delivery evidence to exact canonical target blocks, read-only."""
import collections,csv,json,statistics,sys,urllib.request
from pathlib import Path
out=Path(sys.argv[1]);detail=out/'detail'
candidates={c['candidate_id']:c for c in json.loads((detail/'all-candidates.json').read_text())}
events=json.loads((detail/'events.private.json').read_text());headers={};rows=[]
source_posts={p['hash'] for p in csv.DictReader((out/'canonical-postings.csv').open())}
def header(number):
 if number not in headers:
  request=urllib.request.Request('http://127.0.0.1:18645',json.dumps({'jsonrpc':'2.0','id':1,'method':'eth_getBlockByNumber','params':[hex(number),False]}).encode(),{'Content-Type':'application/json'})
  with urllib.request.urlopen(request,timeout=20) as response:result=json.load(response)
  assert 'error' not in result and result['result'];headers[number]=result['result']
 return headers[number]
for event in events:
 if event['data']['kind']!='delivery':continue
 d=event['data']['detail'];c=candidates[event['candidate_id']]
 assert c['estimator_version']=='companion-input-8'
 number=d['target_block'];block=header(number);tx=d['transaction_hash'];included=tx in block['transactions']
 if included:classification='included_target'
 elif c['canonical_l1_block'] is not None:classification='included_elsewhere'
 elif int(block['timestamp'],16)==d['target_timestamp']:classification='produced_target_without_post'
 elif int(block['timestamp'],16)>d['target_timestamp']:classification='skipped_target_slot'
 else:classification='target_timestamp_mismatch'
 if included:
  assert d['expected_parent_hash']==block['parentHash']
  assert c['canonical_l1_block']==number and c['canonical_l1_hash']==block['hash']
 rows.append({'candidate_id':event['candidate_id'],'post_batch_hash':tx,'source_bearing_canonical_post':tx in source_posts,'classification':classification,'acknowledged':d['acknowledged'],'delivery_ms':d['delivery_ms'],'ack_remaining_ms':d['ack_remaining_ms'],'relay_bundle_hash':d['relay_bundle_hash'],'target_block':number,'target_timestamp':d['target_timestamp'],'expected_parent_hash':d['expected_parent_hash'],'canonical_parent_hash':block['parentHash'],'parent_matches':d['expected_parent_hash']==block['parentHash'],'bundle_transaction_count':d['transaction_count'],'canonical_target_gas_used':int(block['gasUsed'],16),'canonical_target_gas_limit':int(block['gasLimit'],16),'canonical_target_transactions':len(block['transactions'])})
assert rows and len({(r['candidate_id'],r['post_batch_hash']) for r in rows})==len(rows)
with (detail/'delivery-evidence.csv').open('w') as f:
 writer=csv.DictWriter(f,fieldnames=list(rows[0]));writer.writeheader();writer.writerows(rows)
summary={'deliveries':len(rows),'acknowledged':sum(r['acknowledged'] for r in rows),'relay_identifiers':sum(r['relay_bundle_hash'] is not None for r in rows),'classifications':dict(collections.Counter(r['classification'] for r in rows)),'groups':[]}
for name in sorted({r['classification'] for r in rows}):
 group=[r for r in rows if r['classification']==name]
 summary['groups'].append({'classification':name,'count':len(group),'delivery_ms_mean':statistics.mean(r['delivery_ms'] for r in group),'ack_remaining_ms_min':min(r['ack_remaining_ms'] for r in group),'ack_remaining_ms_median':statistics.median(r['ack_remaining_ms'] for r in group),'parent_matches':sum(r['parent_matches'] for r in group),'block_gas_fraction_mean':statistics.mean(r['canonical_target_gas_used']/r['canonical_target_gas_limit'] for r in group)})
(detail/'delivery-summary.json').write_text(json.dumps(summary,indent=2)+'\n');print(json.dumps(summary,indent=2))
