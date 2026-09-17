"""Sanitized capability inventory: metric names/types and label keys only."""
import json
import re

NAME = re.compile(r'[a-zA-Z_:][a-zA-Z0-9_:]*\Z')
KEY = re.compile(r'([a-zA-Z_][a-zA-Z0-9_]*)=')


def inventory(text):
    result, types = {}, {}
    decoder = json.JSONDecoder()
    for line in text.splitlines():
        if line.startswith('# TYPE '):
            name, kind = line[7:].split()
            assert NAME.fullmatch(name) and kind in ('gauge','counter','histogram','summary','untyped')
            types[name] = kind
        if not line or line.startswith('#'):
            continue
        match = re.match(r'([a-zA-Z_:][a-zA-Z0-9_:]*)([ {])', line)
        assert match, 'invalid sample name boundary'
        name = match[1]
        keys = result.setdefault(name, set())
        if match[2] != '{':
            continue
        labels = line[match.end():]
        while not labels.startswith('}'):
            key = KEY.match(labels)
            assert key, 'invalid label key'
            keys.add(key[1])
            value, end = decoder.raw_decode(labels[key.end():])
            assert isinstance(value, str), 'invalid quoted label value'
            labels = labels[key.end()+end:].lstrip()
            if labels.startswith(','):
                labels = labels[1:].lstrip()
            else:
                assert labels.startswith('}'), 'invalid label separator'
    return {'format':'metric sample names/types and label keys only; no label/sample values',
            'types':dict(sorted(types.items())),
            'samples':{key:sorted(value) for key,value in sorted(result.items())}}
