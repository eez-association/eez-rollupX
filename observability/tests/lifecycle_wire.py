"""Synthetic lifecycle v1 records shared by importer and restore fixtures."""
import hashlib
import json


def line(value):
    return json.dumps(value, separators=(',', ':')).encode() + b'\n'


def header(identity, epoch, segment):
    return line({'record': 'header', 'version': 1, 'identity': identity, 'epoch': epoch, 'segment_id': segment,
                 'process_id': 123, 'started_at_unix_ms': 10000, 'segment_created_at_unix_ms': 10001})


def chunks(identity, epoch, sequence, pieces=1, payload=None):
    if payload is None:
        payload = {'observed_at_unix_ms': 10002, 'trace': None,
                   'event': {'event': 'reorg', 'ancestor': {'number': 1, 'hash': '03' * 32}}}
    encoded = json.dumps(payload, separators=(',', ':')).encode()
    step = (len(encoded) + pieces - 1) // pieces
    parts = [encoded[offset:offset + step] for offset in range(0, len(encoded), step)]
    return [line({'record': 'data', 'version': 1, 'identity': identity, 'epoch': epoch, 'sequence': sequence,
                  'chunk_index': index, 'chunk_count': len(parts), 'payload_hex': part.hex(),
                  'complete': index + 1 == len(parts),
                  'digest': hashlib.sha256(encoded).hexdigest() if index + 1 == len(parts) else None})
            for index, part in enumerate(parts)]
