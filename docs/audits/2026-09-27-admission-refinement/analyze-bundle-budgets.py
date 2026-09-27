"""Compare admitted and actual ordered start envelopes against the active block cap."""
import collections,csv,json,statistics,sys
from pathlib import Path
out=Path(sys.argv[1]);detail=out/'detail';posts=list(csv.DictReader((out/'canonical-postings.csv').open()));candidates=json.loads((detail/'all-candidates.json').read_text());events=json.loads((detail/'events.private.json').read_text());snapshot=json.loads((out/'canonical-snapshot.private.json').read_text());by=collections.defaultdict(list)
for e in events:by[e['candidate_id']].append(e['data'])
blocks={b['hash']:b for b in snapshot['blocks']['L1']};num=lambda x:int(x,16);rows=[]
for post in posts:
 c=next(c for c in candidates if c['transaction_hash']==post['hash']);ev=by[c['candidate_id']];summary=next(e['detail'] for e in ev if e['kind']=='summary');est={e['detail']['source_hash']:e['detail'] for e in ev if e['kind']=='source_estimate'};accepted=[e['detail']['source_hash'] for e in ev if e['kind']=='source_accepted'];ordered=[h for h in accepted if est[h]['direction']=='inbound'];prefix=required=0
 for h in ordered:
  e=est[h];required=max(required,prefix+e['companion_declared_gas']);prefix+=e['companion_estimate_gas']
 assert prefix==summary['companion_gas']
 block=blocks[post['block_hash']];txs={t['hash']:t for t in block['transactions']};receipts={r['transactionHash']:r for r in snapshot['receipts']['L1'][block['hash']]}
 assert ordered==[t['hash'] for t in block['transactions'] if t['hash'] in set(ordered)]
 used=num(receipts[post['hash']]['gasUsed']);actual_required=num(txs[post['hash']]['gas'])
 for h in ordered:actual_required=max(actual_required,used+num(txs[h]['gas']));used+=num(receipts[h]['gasUsed'])
 cap=summary['whole_block_cap'];estimate=c['admission_gas']+600000+required
 assert actual_required<=num(block['gasLimit'])
 rows.append({'l1_block':post['block_number'],'sources':post['sources'],'reason':c['admission_stop_reason'],'parent_budget_gas':cap,'actual_block_limit_gas':num(block['gasLimit']),'estimated_bundle_start_gas':estimate,'actual_bundle_used_gas':used,'actual_signed_start_gas':actual_required,'estimate_budget_fraction':estimate/cap,'actual_used_budget_fraction':used/cap,'actual_start_budget_fraction':actual_required/cap,'actual_start_headroom_gas':cap-actual_required,'preparation_ms':c['preparation_ms']})
with (detail/'bundle-budget-per-post.csv').open('w') as f:w=csv.DictWriter(f,fieldnames=list(rows[0]));w.writeheader();w.writerows(rows)
limited=[r for r in rows if r['reason']=='l1_bundle_gas_budget'];summary={'postbatches':len(rows),'bundle_limited_postbatches':len(limited),'parent_budget_min_gas':min(r['parent_budget_gas'] for r in rows),'parent_budget_max_gas':max(r['parent_budget_gas'] for r in rows),'all_actual_signed_start_envelopes_fit':True}
for group,name in [(rows,'all'),(limited,'bundle_limited')]:
 if group:
  summary[name]={k:statistics.mean(r[k] for r in group) for k in ['estimate_budget_fraction','actual_used_budget_fraction','actual_start_budget_fraction']}
(detail/'bundle-budget-review.json').write_text(json.dumps(summary,indent=2)+'\n');print(json.dumps(summary,indent=2))
