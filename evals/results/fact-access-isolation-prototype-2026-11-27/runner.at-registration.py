import hashlib
import json
import os
import statistics
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path

WORK = Path.cwd()
TMP = Path('/private/tmp')
RESULT = WORK / 'evals/results/fact-access-isolation-prototype-2026-11-27'
BIN = TMP / 'memory-quality-heat-prototype-2026-11-27'
DRY = TMP / 'memory-quality-heat-prototype-contract.log'
PROFILE = 'CARGO_PROFILE_TEST_OPT_LEVEL=3'
TEST_NAME = 'isolation_prototype::fact_access_storage_isolation_prototype'
TEST_COMMAND = [str(BIN), '--exact', TEST_NAME, '--ignored', '--nocapture', '--test-threads=1']
TIMEOUT = 120
PAIRS = 20
KEY_COUNTS = [1, 8192]
GATE = 0.50

def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

def write_new(path, data):
    with path.open('x', encoding='utf-8') as stream:
        json.dump(data, stream, indent=2, sort_keys=True)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())

def freeze():
    RESULT.mkdir(parents=True, exist_ok=False)
    files = [
        Path('Cargo.lock'),
        Path('crates/memory-mcp/tests/fact_access_payload.rs'),
        Path('crates/memory-mcp/src/memory/fact_access_store.rs'),
        Path('docs/superpowers/specs/2026-11-27-fact-access-storage-isolation-design.md'),
        Path(__file__),
    ]
    write_new(RESULT / 'registration.json', {
        'schema': 'memory-fact-access-isolation-prototype/v1',
        'registered_before_timed_dispatch': True,
        'binary': str(BIN),
        'binary_sha256': digest(BIN),
        'source_input_sha256': {str(path): digest(path) for path in files},
        'deterministic_preflight_log': str(DRY),
        'deterministic_preflight_sha256': digest(DRY),
        'engine': 'SurrealDB 4.0.1 embedded SurrealKV on native Darwin arm64',
        'profile': PROFILE,
        'build_command': PROFILE + ' cargo test -p memory_mcp --features eval-support --test fact_access_payload --no-run --locked --message-format=json',
        'command': TEST_COMMAND,
        'process_timeout_seconds': TIMEOUT,
        'benchmark_dispatches': 1,
        'runtime_retry_policy': 'Existing SurrealDbClient only. No test-run retries.',
        'body_bytes': 27,
        'key_counts': KEY_COUNTS,
        'key_template': 'cobalt-dimension-{n}',
        'pairs_per_case': PAIRS,
        'fixed_order': '1-key then 8192-key. Pairs 0..19; legacy/isolated for even pairs, isolated/legacy for odd pairs. First access follows each pair.',
        'arms': {
            'legacy': 'Existing production writer, awaited single UPDATE on fact returning NONE.',
            'isolated': 'Awaited exact-key side-record projection then narrow UPDATE on existing side record.',
            'first_access': 'Separately timed existence and create-or-increment transaction; verification and reset outside timer.'
        },
        'metrics': 'Arm wall-clock and combined DB-call microseconds; logical queries and API-boundary SQL+JSON requests/JSON responses, not engine byte counters.',
        'acceptance': {
            'start_complete_rows': 120,
            'deterministic': 'Preflight and reopen pass, exact pre-write and persisted counts/times/source IDs, no absent-row creation, unchanged fact fields.',
            'performance': 'Median combined isolated wall / median legacy write wall <= 0.50 in both AB and BA strata for 8192 keys. Publish cold-first and low-key observations separately.',
            'scope': 'One test-only temporary schema comparison. No production deployment, migration, model, full-history retest, or inferred end-to-end gain.'
        },
        'failure_policy': 'Stop on deterministic runtime fault or external cap, retain all observed phase rows and raw log, do not replace a failed attempt.'
    })

def run():
    registration = RESULT / 'registration.json'
    spec = json.loads(registration.read_bytes())
    assert digest(BIN) == spec['binary_sha256']
    assert digest(DRY) == spec['deterministic_preflight_sha256']
    assert all(digest(Path(path)) == expected for path, expected in spec['source_input_sha256'].items())
    assert spec['command'] == TEST_COMMAND
    assert spec['process_timeout_seconds'] == TIMEOUT
    raw = RESULT / 'prototype.log'
    start = time.monotonic()
    fault = None
    with raw.open('xb') as log:
        try:
            run_result = subprocess.run(TEST_COMMAND, stdout=log, stderr=subprocess.STDOUT, timeout=TIMEOUT, check=False)
            exit_code = run_result.returncode
        except subprocess.TimeoutExpired:
            exit_code = None
            fault = 'external_process_timeout'
    wall_seconds = time.monotonic() - start
    events = [
        json.loads(line.split('HEAT_PROTOTYPE ', 1)[1])
        for line in raw.read_text(errors='replace').splitlines()
        if 'HEAT_PROTOTYPE ' in line
    ]
    attempts = [event for event in events if event['event'] == 'start']
    completed = [event for event in events if event['event'] == 'complete']
    actual = Counter((row['index_keys'], row['pair'], row['arm']) for row in completed)
    planned = [(keys, pair, arm) for keys in KEY_COUNTS for pair in range(PAIRS)
               for arm in (('legacy', 'isolated', 'first_access') if pair % 2 == 0
                           else ('isolated', 'legacy', 'first_access'))]
    attempt_order_ok = [(row['index_keys'], row['pair'], row['arm']) for row in attempts] == planned
    preflight = {'event': 'contract_preflight', 'status': 'passed'} in events
    reopen = {'event': 'contract_reopen', 'status': 'passed'} in events
    contract_ok = exit_code == 0 and preflight and reopen and actual == Counter(planned) and attempt_order_ok
    medians = {}
    high_key_pass = False
    if contract_ok:
        by_key = {(row['index_keys'], row['pair'], row['arm']): row for row in completed}
        for keys in KEY_COUNTS:
            for parity, order in enumerate(('AB', 'BA')):
                samples = {
                    arm: statistics.median(by_key[keys, pair, arm]['wall_us'] for pair in range(parity, PAIRS, 2))
                    for arm in ('legacy', 'isolated', 'first_access')
                }
                samples['isolated_over_legacy'] = samples['isolated'] / samples['legacy']
                medians[f'{keys}_keys_{order}'] = samples
        high_key_pass = all(medians[f'8192_keys_{order}']['isolated_over_legacy'] <= GATE for order in ('AB', 'BA'))
    if fault is None and not contract_ok:
        fault = 'nonzero_exit_or_missing_registered_observations'
    write_new(RESULT / 'observation.json', {
        'registration_sha256': digest(registration),
        'binary_sha256': digest(BIN),
        'raw_log_sha256': digest(raw),
        'exit_code': exit_code,
        'wall_seconds': wall_seconds,
        'fault': fault,
        'attempted': len(attempts),
        'completed': len(completed),
        'deterministic_and_completion_gate_passed': contract_ok,
        'high_key_gate_passed': high_key_pass,
        'median_wall_us': medians,
        'events': events
    })
    print(json.dumps({'started': len(attempts), 'completed': len(completed), 'exit_code': exit_code,
        'wall_seconds': wall_seconds, 'fault': fault, 'deterministic_and_completion_gate_passed': contract_ok,
        'high_key_gate_passed': high_key_pass, 'median_wall_us': medians}, indent=2))

if sys.argv[1:] == ['freeze']:
    freeze()
elif sys.argv[1:] == ['run']:
    run()
else:
    raise SystemExit('expected freeze or run')
