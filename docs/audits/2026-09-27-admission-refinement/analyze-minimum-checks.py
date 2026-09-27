"""Reconcile cheap refusals and full simulation counters for the measured cohort."""
import collections,csv,json,sys
from pathlib import Path
out=Path(sys.argv[1]);detail=out/'detail';events=json.loads((detail/'events.private.json').read_text());candidates=json.loads((detail/'all-candidates.json').read_text());by=collections.defaultdict(list)
for event in events:by[event['candidate_id']].append(event['data'])
rows=[];counter=collections.Counter();simulations=0;full_refusals=0;cheap_rows=[]
for c in candidates:
 ev=by[c['candidate_id']];summary=next((e['detail'] for e in ev if e['kind']=='summary'),None)
 if summary is None:continue
 cheap=[e for e in ev if e['kind']=='source_capacity_check'];estimates=[e for e in ev if e['kind']=='source_estimate'];estimate_hashes={e['detail']['source_hash'] for e in estimates}
 assert len(cheap)==summary['minimum_check_count']==c['minimum_check_count']
 assert summary['simulation_count']==c['simulation_count'] and summary['simulation_count']>=len(estimates)
 for e in cheap:
  d=e['detail'];assert d['basis']=='minimum_admission_cost' and d['source_hash'] not in estimate_hashes
  assert any(a['kind']=='source_outcome' and a['detail']['source_hash']==d['source_hash'] and a['detail']['reason']==d['constraint'] for a in ev)
  counter[d['constraint']]+=1;cheap_rows.append({'candidate_id':c['candidate_id'],**d})
 full=sum(not e['detail']['gas_gate_accepted'] for e in estimates);full_refusals+=full;simulations+=summary['simulation_count']
 rows.append({'candidate_id':c['candidate_id'],'canonical_l1_block':c['canonical_l1_block'],'accepted':c['accepted'],'minimum_check_count':len(cheap),'simulation_count':summary['simulation_count'],'full_estimate_count':len(estimates),'full_gas_refusals':full,'preparation_ms':c['preparation_ms'],'stop_reason':c['admission_stop_reason']})
summary={'candidates_with_summary':len(rows),'cheap_refusals':len(cheap_rows),'cheap_refusals_by_reason':dict(counter),'full_simulations':simulations,'full_gas_refusals':full_refusals,'cheap_fraction_of_gas_refusals':len(cheap_rows)/(len(cheap_rows)+full_refusals) if cheap_rows else 0,'cheap_rows_without_full_estimate':True,'source_outcomes_and_candidate_counters_reconciled':True}
for name,data in [('minimum-checks-summary.json',summary),('minimum-check-candidates.json',rows)]: (detail/name).write_text(json.dumps(data,indent=2)+'\n')
for name,data in [('minimum-check-candidates.csv',rows),('minimum-checks.csv',cheap_rows)]:
 if data:
  with (detail/name).open('w') as f:w=csv.DictWriter(f,fieldnames=list(data[0]));w.writeheader();w.writerows(data)
print(json.dumps(summary,indent=2))
