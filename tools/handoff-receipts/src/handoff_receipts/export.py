"""JSONL export and optional native Polylogue delivery-ledger bridge."""
import json
import os
import subprocess
import uuid
from pathlib import Path
from .common import canonical, no_sensitive, private_dir, write_bytes, write_json, parse_timestamp
from .query import inspect

PIN = '3fd060daad3b2893f8394eae7cfa3c4c15cb4840'


def export_rows(store, attempts):
    result = []
    for aid in attempts:
        row = inspect(store, aid)
        source, request, state, body = store.load(aid)
        result.append({'version': 1, 'source': source, 'request': request, 'body': body.decode(),
                       'state': state, 'receipt': row})
    return result


def jsonl(store, attempts, output):
    data = ''.join(canonical(row) + '\n' for row in export_rows(store, attempts))
    no_sensitive(data)
    write_bytes(Path(output), data.encode())
    return {'format': 'jsonl', 'exported': len(attempts)}


def polylogue(store, attempts, output, bridge):
    if not bridge or not Path(bridge).is_file() or not os.access(bridge, os.X_OK):
        raise ValueError('Polylogue export requires an executable Rust bridge; see bridge/README.md')
    rows = export_rows(store, attempts)
    # Native recipient IDs are mandatory; do not fabricate a session for unknowns.
    if any(row['receipt']['thread_id'] == 'unknown' for row in rows):
        raise ValueError('Polylogue cannot import an attempt with unknown thread ID; use JSONL')
    output = Path(output).absolute()
    if output.exists():
        raise ValueError('Polylogue export destination must not exist')
    private_dir(output)
    writes, identities = [], []
    for row in rows:
        source, receipt = row['source'], row['receipt']
        identity = str(uuid.uuid5(uuid.NAMESPACE_URL, 'handoff-receipts:' + source['attempt_id']))
        writes.append({'request_id': identity, 'at_ms': int(parse_timestamp(source['captured_at']).timestamp() * 1000),
                       'write': {'write': 'deliver', 'recipient_kind': 'session',
                                 'recipient_id': (receipt['engine'] + ':' if receipt['engine'] in ('claude', 'agy') else '') + receipt['thread_id'], 'sender': 'cli', 'event': 'advisory',
                                 'text': row['body'], 'refs': [source['path'], 'sha256:' + source['sha256']], 'left_out': []}})
        identities.append({'attempt_id': source['attempt_id'], 'delivery_id': identity})
        if receipt['facts']['target_input_confirmed'] is True:
            writes.append({'request_id': identity + '-delivered', 'at_ms': writes[-1]['at_ms'],
                           'write': {'write': 'delivery_event', 'delivery': identity, 'what': 'delivered',
                                     'detail': 'Exact target input re-read; exposure/use unknown. See common-E.jsonl.'}})
    write_json(output / 'native-input.json', writes)
    # Native child-created archive files must also be private from creation.
    previous = os.umask(0o077)
    try:
        result = subprocess.run([str(Path(bridge).absolute()), str(output / 'native-input.json'), str(output / 'archive')],
                                capture_output=True, timeout=60, env={'PATH': '/usr/bin:/bin', 'HOME': str(output)})
    finally:
        os.umask(previous)
    if result.returncode:
        raise ValueError('Polylogue native import/readback failed (raw diagnostics suppressed)')
    exported = json.loads(result.stdout)
    no_sensitive(canonical(exported))
    if len(exported) != len(rows) or sorted(r['text'] for r in exported) != sorted(r['body'] for r in rows):
        raise ValueError('Polylogue native readback mismatch')
    write_json(output / 'deliveries.json', exported)
    write_json(output / 'attempt-map.json', identities)
    jsonl(store, attempts, output / 'common-E.jsonl')
    assessment = {'format': 'polylogue', 'exported': len(rows), 'native_readback': True,
                  'bridge_contract_revision': PIN, 'bridge_revision_verified': False,
                  'supplement': 'common-E.jsonl', 'omissions': ['process/RPC lifecycle', 'turn results', 'retry grouping', 'selected line inventory']}
    write_json(output / 'assessment.json', assessment)
    return assessment
