#!/usr/bin/env python3
"""Offline sizing experiment; does not modify or submit protocol data.

Run from the repository root. Requires Python 3 only; no general-purpose compression is used.
Input artifacts are local captured evidence, not included in git. Output records
their hashes. This is a sizing parser, not an independent KZG/consensus verifier.
"""
import argparse
import hashlib
import json
import math
import statistics
from pathlib import Path

CAPACITY = 126976


def rlp_encode(value):
    if isinstance(value, list):
        data = b''.join(map(rlp_encode, value))
        offset = 192
    else:
        data = value
        if len(data) == 1 and data[0] < 128:
            return data
        offset = 128
    if len(data) < 56:
        return bytes([offset + len(data)]) + data
    length = len(data).to_bytes((len(data).bit_length() + 7) // 8, 'big')
    return bytes([offset + 55 + len(length)]) + length + data


def rlp_decode(data, pos=0):
    start = pos
    tag = data[pos]
    pos += 1
    if tag < 128:
        return bytes([tag]), pos
    is_list = tag >= 192
    offset = 192 if is_list else 128
    length = tag - offset
    if length > 55:
        n = length - 55
        length = int.from_bytes(data[pos:pos+n], 'big')
        pos += n
    end = pos + length
    assert end <= len(data)
    if is_list:
        value = []
        while pos < end:
            item, pos = rlp_decode(data, pos)
            value.append(item)
        assert pos == end
    else:
        value = data[pos:end]
    assert rlp_encode(value) == data[start:end], 'noncanonical RLP'
    return value, end


def varint(value):
    out = bytearray()
    while value >= 128:
        out.append((value & 127) | 128)
        value >>= 7
    out.append(value)
    return bytes(out)


def read_bytes(data, pos):
    length = 0
    for shift in range(0, 35, 7):
        b = data[pos]
        pos += 1
        length |= (b & 127) << shift
        if b < 128:
            assert length <= 0xffffffff and pos + length <= len(data)
            return data[pos:pos+length], pos + length
    raise ValueError('invalid varint')


def unpack(blobs):
    out = bytearray()
    for blob in blobs:
        assert len(blob) == 131072
        for p in range(0, len(blob), 32):
            assert blob[p] == 0
            out.extend(blob[p+1:p+32][::-1])
    assert out[:2] == b'\x00\x02'
    operations, pos = read_bytes(out, 10)
    semantics_start = pos
    while True:
        tag = out[pos]
        pos += 1
        if tag == 1:
            assert not any(out[pos:])
            return bytes(out[:pos]), bytes(operations), bytes(out[semantics_start:pos])
        if tag in (3, 4, 5, 6, 7):
            pos += {3: 8, 4: 88, 5: 56, 6: 0, 7: 0}[tag]
            _, pos = read_bytes(out, pos)
        else:
            assert tag in (8, 9, 10), tag


def byte_vector(items):
    assert isinstance(items, list)
    values = [int.from_bytes(v, 'big') for v in items]
    assert all(v <= 255 for v in values)
    # alloy encodes zero integer as the empty RLP string.
    assert all(v == (b'' if n == 0 else bytes([n])) for v, n in zip(items, values))
    return bytes(values)


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--corpus', type=Path, default=Path('artifacts/protocol-parallel-final.8ztteARM/results/protocol-e2e/evidence'))
    parser.add_argument('--audit', type=Path, default=Path('artifacts/chiado-result-reserves-20260916/candidate/canonical-audit.json'))
    args = parser.parse_args()
    samples, inputs, seen = [], {}, set()
    for path in sorted(args.corpus.glob('*.blob-sidecars.json')):
        evidence_path = path.with_name(path.name.replace('.blob-sidecars', ''))
        evidence = json.loads(evidence_path.read_text())['blob_da']
        assert evidence['verified'] is True
        blobs = [bytes.fromhex(x.removeprefix('0x')) for x in json.loads(path.read_text())['data']]
        assert len(blobs) == evidence['blob_count']
        digest = hashlib.sha256(b''.join(blobs)).hexdigest()
        inputs[str(path)] = sha(path)
        inputs[str(evidence_path)] = sha(evidence_path)
        if digest in seen:
            continue
        seen.add(digest)
        stream, operations, semantics = unpack(blobs)
        assert len(operations) == evidence['payload']['bytes']
        assert operations[0] == 2
        body, end = rlp_decode(operations, 1)
        assert end == len(operations) and len(body) == 3
        blocks = [byte_vector(x) for x in body[0]]
        entries = [byte_vector(x) for x in body[1]]
        assert len(blocks) == evidence['payload']['block_count']
        assert [len(b) for b in blocks] == [b['rlp_bytes'] for b in evidence['payload']['blocks']]
        compact = b'\x03' + rlp_encode([blocks, body[1], body[2]])
        decoded, end = rlp_decode(compact, 1)
        assert end == len(compact) and decoded == [blocks, body[1], body[2]]
        headers = 0
        decoded_blocks = []
        empty_block_sizes = []
        for block in blocks:
            decoded_block, end = rlp_decode(block)
            assert end == len(block)
            decoded_blocks.append(decoded_block)
            if not decoded_block[1]:
                empty_block_sizes.append(len(rlp_encode(body[0][len(decoded_blocks)-1])))
            headers += len(rlp_encode(decoded_block[0]))
        # Lossless structural delta experiment. First header is complete;
        # each subsequent header gives a field bitmap and exact changed values.
        # This experiment requires equal header arity within a span.
        first_header = decoded_blocks[0][0]
        previous = first_header
        changes = []
        for block in decoded_blocks[1:]:
            header = block[0]
            assert len(header) == len(previous), 'split span at header-layout changes'
            indices = [i for i, v in enumerate(header) if v != previous[i]]
            bitmap = sum(1 << i for i in indices)
            changes.append([bitmap.to_bytes((bitmap.bit_length()+7)//8, 'big'), [header[i] for i in indices]])
            previous = header
        span_body = [first_header, changes, [block[1:] for block in decoded_blocks], body[1], body[2]]
        span = b'\x04' + rlp_encode(span_body)
        decoded_span, end = rlp_decode(span, 1)
        assert end == len(span) and decoded_span == span_body
        header = decoded_span[0]
        restored = [rlp_encode([header] + decoded_span[2][0])]
        for (bitmap_bytes, values), tail in zip(decoded_span[1], decoded_span[2][1:]):
            bitmap = int.from_bytes(bitmap_bytes, 'big')
            iterator = iter(values)
            header = [next(iterator) if bitmap & (1 << i) else v for i, v in enumerate(header)]
            assert next(iterator, None) is None
            restored.append(rlp_encode([header] + tail))
        assert restored == blocks, 'every original block must round-trip byte-for-byte'
        assert rlp_encode(decoded[1:]) == rlp_encode(body[1:]), 'preserve entry/group encodings'
        assert rlp_encode(decoded_span[3:]) == rlp_encode(body[1:]), 'preserve entry/group encodings'
        # Only replace the opaque operations field. Cross-chain stream bytes,
        # including the final carrier marker, are copied without reinterpretation.
        for candidate in (compact, span):
            new_stream = stream[:10] + varint(len(candidate)) + candidate + semantics
            recovered, semantic_offset = read_bytes(new_stream, 10)
            assert recovered == candidate and new_stream[semantic_offset:] == semantics
        def native_size(payload):
            return 10 + len(varint(len(payload))) + len(payload) + len(semantics)
        assert native_size(operations) == len(stream)
        row = dict(empty_block_encoded_sizes=empty_block_sizes, scenario=path.name.split('.')[0], blob_sha256=digest, current_blobs=len(blobs), current_bytes=len(stream), operations_bytes=len(operations), semantic_bytes=len(semantics)-1, block_count=len(blocks), raw_block_bytes=sum(map(len, blocks)), raw_header_bytes=headers, raw_entry_bytes=sum(map(len, entries)), compact_bytes=native_size(compact), compact_blobs=math.ceil(native_size(compact)/CAPACITY), span_bytes=native_size(span), span_blobs=math.ceil(native_size(span)/CAPACITY))
        samples.append(row)
    assert samples, 'empty corpus'
    total = sum(r['current_bytes'] for r in samples)
    summary = dict(samples=len(samples), current_bytes=total, current_blobs=sum(r['current_blobs'] for r in samples))
    empty_sizes = [size for row in samples for size in row['empty_block_encoded_sizes']]
    summary['empty_blocks'] = dict(count=len(empty_sizes), encoded_min=min(empty_sizes), encoded_median=statistics.median(empty_sizes), encoded_max=max(empty_sizes), illustrative_six_blob_two_second_capacity_minutes=6*CAPACITY/statistics.median(empty_sizes)*2/60)
    for form in ['compact', 'span']:
        size = sum(r[form + '_bytes'] for r in samples)
        summary[form] = dict(bytes=size, reduction_percent=100*(1-size/total), blobs=sum(r[form + '_blobs'] for r in samples))
    audit = json.loads(args.audit.read_text())
    inputs[str(args.audit)] = sha(args.audit)
    chiado = {}
    for name, posts in [('all_posts', audit['posts']), ('source_posts', [p for p in audit['posts'] if p['sourceCount']])]:
        measured = [p for p in posts if p['encodedBytes'] is not None]
        size = sum(p['encodedBytes'] for p in measured)
        blobs = sum(p['physicalBlobs'] for p in measured)
        chiado[name] = dict(posts=len(posts), total_blobs=sum(p['physicalBlobs'] for p in posts), measured_posts=len(measured), measured_blobs=blobs, encoded_bytes=size, paid_capacity_fill_percent=100*size/(blobs*CAPACITY), missing_size_hashes=[p['hash'] for p in posts if p['encodedBytes'] is None], sizes=[p['encodedBytes'] for p in measured])
    chiado['all_paid_costs'] = audit['capturedPaidPostCosts']
    chiado['admission_reason_counts'] = audit['admissionReasonCounts']
    print(json.dumps(dict(capacity=CAPACITY, inputs_sha256=inputs, corpus_summary=summary, chiado=chiado, samples=samples), indent=2))


if __name__ == '__main__':
    main()
