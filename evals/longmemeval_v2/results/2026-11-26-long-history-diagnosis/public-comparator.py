"""Bounded formation/read comparator through the existing public adapter.

Run from repository root with --output /path/to/new-receipt.json; optionally --binary /path/to/memory_mcp.
No model calls; temporary database; 15s per command; <= 8 CLI calls.
"""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[4]))
from evals.longmemeval_v2.memory_mcp_backend import MemoryMcpBackend
from evals.longmemeval_v2.text_cohort import verified_span
from evals.longmemeval_v2.reviewed_span_coverage import decode_saved_text

root = Path(__file__).resolve().parents[4]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, default=root / 'target/release/memory_mcp')
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--binary-source-revision', default='unknown_unverified_build_revision')
args = parser.parse_args()
out = args.output
if out.exists():
    parser.error('output already exists; preserve the original receipt and choose a new path')
binary = args.binary.resolve()
cohort = json.loads((root / 'evals/longmemeval_v2/small-web-text-cohort.json').read_text())
source = json.loads((root / 'target/c4-native/cohort/trajectories.json').read_text())
review = cohort['reviews']['6d975a08']
trajectory = next(item for item in source if item['id'] == review['trajectory_id'])
span = verified_span(trajectory, review)
history = [
    {'id': 'reviewed-directory-miniature', 'states': [{'state_index': 0, 'accessibility_tree': span}]},
    {'id': 'unrelated-shipping', 'states': [{'state_index': 0, 'accessibility_tree': 'Shipping dashboard: parcel delivery takes five business days. Warehouse contact: Robin.'}]},
    {'id': 'unrelated-weather', 'states': [{'state_index': 0, 'accessibility_tree': 'Weather dashboard: Oslo forecast is cloudy. Temperature: seven degrees.'}]},
]
report = {'schema': 'memory-mcp-bounded-public-long-history-comparator/v1', 'host': {'architecture': platform.machine(), 'os': platform.system()}, 'binary_source_revision': args.binary_source_revision, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'source_span_sha256': review['span_sha256'], 'history': history, 'history_sha256': hashlib.sha256(json.dumps(history,sort_keys=True).encode()).hexdigest(), 'limits': {'cli_timeout_seconds': 15, 'maximum_cli_calls': 8, 'remote_calls': 0, 'reader_model_calls': 0}, 'operations': [], 'interpretation': 'Miniature feasibility diagnostic only, not a seven-case rerun or representative long-history benchmark.'}
query = 'On the All forums directory page, what are the three browse tabs shown above the forum list?'
report['query'] = query
started = time.monotonic()
with tempfile.TemporaryDirectory(prefix='long-history-public-miniature-') as store:
    backend = MemoryMcpBackend({'db_path': str(Path(store)/'store'), 'binary': str(binary), 'cli_timeout_seconds': 15})
    operations = [('insert_required', lambda: backend.insert(history[0])), ('query_required_alone', lambda: backend.query(query)), ('insert_distractor_shipping', lambda: backend.insert(history[1])), ('insert_distractor_weather', lambda: backend.insert(history[2])), ('query_with_distractors', lambda: backend.query(query))]
    for name, action in operations:
        begin = time.monotonic()
        receipt = {'operation': name}
        try:
            value = action()
            receipt.update(status='completed', output=value)
            if name.startswith('query'):
                receipt['complete_reviewed_span'] = any(span in decode_saved_text(item['value']) for item in value)
                receipt['marker_presence'] = {marker: any(marker in item['value'] for item in value) for marker in ['Forums','Tags','Alphabetical']}
        except subprocess.TimeoutExpired:
            receipt.update(status='timeout', timeout_seconds=15)
        except Exception as error:
            receipt.update(status='error', error_type=type(error).__name__)
        receipt['elapsed_seconds'] = round(time.monotonic()-begin,3)
        report['operations'].append(receipt)
        report['elapsed_seconds'] = round(time.monotonic()-started,3)
        report['adapter_stats'] = backend.stats
        out.write_text(json.dumps(report,indent=2)+'\n')
        if receipt['status'] != 'completed':
            report['result'] = 'inconclusive_deadline_or_error; no successful retry selected'
            break
    else:
        report['result'] = 'completed; compare required-alone and distractor contexts without inferring unobserved internal stages'
    out.write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({'result': report['result'], 'elapsed_seconds': report['elapsed_seconds'], 'operations': [{k:v for k,v in item.items() if k != 'output'} for item in report['operations']]},indent=2))
