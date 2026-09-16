#!/usr/bin/env python3
"""Extract public canonical accounting facts from a saved, complete EEZ audit.

No network calls. Whitelist receipt/identity fields; exclude signed envelopes,
calldata, keys, deployment config and arbitrary diagnostic text.
"""
import argparse
import hashlib
import json
import pathlib


def extract(audit_bytes, evidence_bytes):
    audit, evidence = json.loads(audit_bytes), json.loads(evidence_bytes)
    evidence_digest = hashlib.sha256(evidence_bytes).hexdigest()
    if not audit['completeAudit'] or audit['gaps'] or not evidence['revalidated']:
        raise ValueError('fixture requires a complete, revalidated canonical audit')
    if audit['canonicalEvidenceSha256'] != evidence_digest:
        raise ValueError('audit does not identify the supplied evidence bytes')
    receipt_fields = ('transactionHash', 'blockHash', 'blockNumber', 'transactionIndex',
                      'type', 'status', 'gasUsed', 'effectiveGasPrice', 'blobGasUsed', 'blobGasPrice')
    receipts = []
    l1_cost = 0
    for chain in ('l1', 'l2'):
        for tx_hash, raw in sorted(evidence['receipts'][chain].items()):
            if raw['transactionHash'] != tx_hash or evidence['canonicalByNumber'][chain][str(int(raw['blockNumber'], 16))] != raw['blockHash']:
                raise ValueError('receipt inclusion differs from revalidated canonical identity')
            transaction = evidence['transactions'][chain][tx_hash]
            if transaction['hash'] != tx_hash or transaction['blockHash'] != raw['blockHash']:
                raise ValueError('transaction and receipt inclusion differ')
            receipts.append({'chain': chain, 'receipt': {key: raw[key] for key in receipt_fields if key in raw},
                'transaction': {**{key: transaction[key] for key in ('hash', 'from', 'to', 'nonce', 'type')},
                    'method_selector': transaction['input'][:10],
                    'blob_versioned_hashes': transaction.get('blobVersionedHashes', [])}})
            if chain == 'l1':
                cost = int(raw['gasUsed'], 16) * int(raw['effectiveGasPrice'], 16)
                if int(raw['type'], 16) == 3:
                    cost += int(raw['blobGasUsed'], 16) * int(raw['blobGasPrice'], 16)
                l1_cost += cost
    if str(l1_cost) != audit['capturedPaidPostCosts']['totalWei']:
        raise ValueError('receipt charges differ from saved audit totals')
    posts = []
    for post in audit['posts']:
        handoff = post['evidence'].get('handoff')
        members = post['orderedHandoffSources']
        companions = [row['hash'] for row in post['cost']['companions']]
        if not set(companions) <= set(members):
            raise ValueError('companion charge lacks explicit source membership')
        posts.append({'transaction_hash': post['hash'], 'generation': post['generation'],
            'block_hash': post['l1BlockHash'], 'source_hashes': members, 'companion_hashes': companions,
            'membership_provenance': None if handoff is None else {
                **{key: handoff[key] for key in ('file', 'line', 'timestamp')},
                'assurance': 'node_reconciled',
                'reported_prepared_post_hash': handoff['fields']['post_batch_hash'],
                'canonical_post_hash': post['hash'], 'native_generation': post['generation']},
            'physical_blobs': post['physicalBlobs'], 'reported_encoded_bytes': post['encodedBytes'],
            'measurement_assurance': 'composer_reported_unverified',
            'expected_total_wei': post['cost']['bundleTotalWei']})
    return {'version': 1, 'provenance': {
            'audit_sha256': hashlib.sha256(audit_bytes).hexdigest(), 'canonical_evidence_sha256': evidence_digest,
            'analyzer_sha256': audit['analyzerSha256'], 'receipt_verifier_sha256': audit['receiptVerifierSha256'],
            'captured_at': evidence['capturedAt'],
            'scope': 'Saved result-reserves accounting corpus; not a contiguous bootstrap RPC source or independent cross-chain replay proof.'},
        'chain_ids': evidence['chainId'], 'receipts': receipts, 'posts': posts,
        'sources': [{key: row[key] for key in ('hash', 'chain', 'direction', 'post', 'ownerGeneration', 'reason', 'sourceBlockHash')} for row in audit['transactions']],
        'expected_l1_costs': audit['capturedPaidPostCosts']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--audit', type=pathlib.Path, required=True)
    parser.add_argument('--evidence', type=pathlib.Path, required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    args = parser.parse_args()
    fixture = extract(args.audit.read_bytes(), args.evidence.read_bytes())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(fixture, indent=2, sort_keys=True) + '\n')
    print(f'Captured {len(fixture["receipts"])} receipt facts and {len(fixture["posts"])} explicit post memberships.')


if __name__ == '__main__':
    main()
