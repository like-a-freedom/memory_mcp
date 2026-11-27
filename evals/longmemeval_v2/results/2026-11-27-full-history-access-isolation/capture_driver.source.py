"""Run one registered pair per original case on fresh, local closed-store clones."""
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import time

ROOT = Path('/Users/solovey/Documents/dev/memory_mcp')
RESULTS = ROOT / 'evals/longmemeval_v2/results'
OUT = RESULTS / '2026-11-27-full-history-access-isolation'
RAW = Path('/tmp/memory-quality-full-100-2026-11-27/observations-heat-isolation')
SOURCE = ROOT / 'data/eval-snapshots/full-history-2026-11-27/store'
UPGRADED = Path('/tmp/memory-quality-full-100-2026-11-27/access-isolation-upgraded/store')
RESTORE = RESULTS / '2026-11-27-full-history-restore'
BUILD_INPUT = RESTORE / 'registration.json'
READ_INPUT = RESTORE / 'reads-registration.json'
RETENTION = RESTORE / 'retention.json'
VERIFIER = ROOT / 'evals/longmemeval_v2/snapshot_manifest.py'
NEW = Path('/tmp/memory-quality-history-v5-2026-11-27')
OLD = Path('/tmp/memory-quality-history-heatdiag-2026-11-27')
READ_TEST = 'full_history_registered_case_retains_reviewed_source'
AUDIT_TEST = 'full_history_formation_inventory'
UPGRADE_TEST = 'full_history_upgrade_fact_access_statistics'
STRICT_ASSERTION = b'full-history retrieval must retain the complete reviewed source block'
CASES = ('0c6b0c60', '6d975a08', '89a20f48', '922a0c56', 'd6da3938', 'f43926d9', 'f61a096f')
LIMITS = dict(read_seconds=30, child_seconds=45, budget=5, compact=False, upgrade_seconds=300, audit_seconds=300)


def sha(path):
    with path.open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def hashes(path):
    return {str(file.relative_to(path)): sha(file) for file in sorted(path.rglob('*')) if file.is_file()}


def load(path):
    return json.loads(path.read_text(encoding='utf-8'))


def save(path, value):
    with path.open('x', encoding='utf-8') as file:
        json.dump(value, file, ensure_ascii=True, indent=2, allow_nan=False)
        file.write('\n')
        file.flush()
        os.fsync(file.fileno())


def record(file, event, **data):
    now = datetime.now(timezone.utc).isoformat()
    file.write(json.dumps({'event': event, 'at_utc': now, **data}, ensure_ascii=True, allow_nan=False) + '\n')
    file.flush()
    os.fsync(file.fileno())


def preflight():
    identity = load(OUT / 'execution-identity.json')
    protocol_path = OUT / 'protocol.json'
    for name, path in {
        'paired_driver': Path(__file__), 'protocol': protocol_path, 'build_registration': BUILD_INPUT,
        'read_registration': READ_INPUT, 'retention_manifest': RETENTION, 'snapshot_verifier': VERIFIER,
        'candidate_observer': NEW, 'legacy_observer': OLD,
    }.items():
        if sha(path) != identity[f'{name}_sha256']:
            raise RuntimeError(f'frozen {name} identity changed')
    protocol = load(protocol_path)
    build = load(BUILD_INPUT)
    reads = load(READ_INPUT)
    retention = load(RETENTION)
    if (platform.machine() != 'arm64' or protocol['limits'] != LIMITS
            or tuple(protocol['cases']) != CASES or tuple(reads['cases']) != CASES
            or protocol['scheduled_pairs'] != 7 or protocol['scheduled_reads'] != 14
            or protocol['source_trajectories'] != 100 or len(build['ordered_trajectory_ids']) != 100
            or protocol['source_segments'] != 151 or build['expected_episode_count'] != 151
            or len(build['segments']) != 151 or build['architecture'] != 'arm64'
            or reads['limits'] != {'read_seconds': 30, 'child_seconds': 45, 'models': False}
            or protocol['legacy_observer'] != str(OLD)
            or protocol['legacy_observer_sha256'] != identity['legacy_observer_sha256']):
        raise RuntimeError('frozen cases, corpus, architecture, or operating limits differ')
    source_ids = {segment['source_id'] for segment in build['segments']}
    if (len(source_ids) != 151 or any(case['source_id'] not in source_ids for case in reads['cases'].values())
            or any(not case['required_serialized_span'] for case in reads['cases'].values())):
        raise RuntimeError('read cases do not reference the registered source inventory')
    registered_paths = {'input': SOURCE, 'source_registration': BUILD_INPUT, 'read_registration': READ_INPUT}
    if any(ROOT / protocol[field] != path for field, path in registered_paths.items()):
        raise RuntimeError('registered input paths differ')
    expected = {name: meta['sha256'] for name, meta in retention['files'].items()}
    if (retention['snapshot_directory'] != str(SOURCE) or not expected
            or hashes(SOURCE) != expected or sum((SOURCE/name).stat().st_size for name in expected) != retention['total_bytes']):
        raise RuntimeError('retained source snapshot differs from its frozen manifest')
    return build, reads, expected


def argv(binary, test):
    return [str(binary), test, '--exact', '--ignored', '--nocapture']


def child_environment(store, registration=None, case=None):
    env = {key: os.environ[key] for key in ('PATH', 'TMPDIR', 'SYSTEMROOT', 'WINDIR', 'TEMP', 'TMP') if key in os.environ}
    env.update(MEMORY_FULL_HISTORY_STORE=str(store), RUST_LOG='error')
    if registration is not None:
        env['MEMORY_FULL_HISTORY_INPUT'] = str(registration)
    if case is not None:
        env['MEMORY_FULL_HISTORY_CASE'] = case
    return env


def finite_duration(value):
    return type(value) in (int, float) and math.isfinite(value) and value >= 0


def json_float(token):
    value = float(token)
    if not math.isfinite(value):
        raise ValueError('nonfinite JSON number')
    return value


def parse_raw(raw):
    events, malformed = [], []
    strict_assertion = False
    with raw.open('rb') as file:
        for number, line in enumerate(file, 1):
            if line.rstrip(b'\r\n') == STRICT_ASSERTION:
                strict_assertion = True
            if not line.startswith(b'FULL_HISTORY_'):
                continue
            name, _, body = line.partition(b' ')
            event = name.strip().decode('ascii', errors='replace')
            try:
                data = json.loads(body.decode('utf-8'), parse_float=json_float, parse_constant=json_float)
                if not line.endswith(b'\n') or not isinstance(data, dict):
                    raise ValueError('incomplete or non-object event frame')
            except (UnicodeDecodeError, ValueError):
                malformed.append({'line': number, 'event': event})
                continue
            events.append((event, data))
    return events, malformed, strict_assertion


def run_child(command, env, raw, seconds):
    started = time.monotonic()
    code = fault = None
    external_timeout = False
    with raw.open('xb') as log:
        try:
            with subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT) as child:
                try:
                    code = child.wait(timeout=max(0, seconds - (time.monotonic() - started)))
                except subprocess.TimeoutExpired:
                    external_timeout = True
                    child.kill()
                    code = child.wait(timeout=10)
        except (OSError, subprocess.TimeoutExpired) as error:
            fault = type(error).__name__
    elapsed = time.monotonic() - started
    events, malformed, strict_assertion = parse_raw(raw)
    return {
        'command': command, 'exit_code': code, 'external_timeout': external_timeout,
        'elapsed_seconds': elapsed, 'process_fault': fault, 'malformed_events': malformed,
        'strict_span_assertion': strict_assertion, 'raw_log': str(raw), 'raw_sha256': sha(raw),
        'raw_bytes': raw.stat().st_size,
    }, events


def invoke_clone(binary, snapshot, test, registration, raw, seconds, case=None):
    with tempfile.TemporaryDirectory(prefix='memory-quality-restored-read-') as tmp:
        started = time.monotonic()
        store = Path(tmp) / 'store'
        shutil.copytree(snapshot, store)
        copy_seconds = time.monotonic() - started
        meta, events = run_child(argv(binary, test), child_environment(store, registration, case), raw, seconds)
    return {'copy_seconds': copy_seconds, **meta}, events


def bodies(events, event):
    return [body for name, body in events if name == event]


def malformed_frame(meta, *events):
    return any(row['event'] in events for row in meta['malformed_events'])


def passed_process(meta, limit):
    return (meta['exit_code'] == 0 and not meta['external_timeout'] and meta['process_fault'] is None
            and finite_duration(meta['elapsed_seconds']) and meta['elapsed_seconds'] <= limit)


def classify_read(meta, events, case, required_span):
    starts = bodies(events, 'FULL_HISTORY_START')
    finals = bodies(events, 'FULL_HISTORY_RECEIPT')
    failures = bodies(events, 'FULL_HISTORY_FAILURE')
    partial = bodies(events, 'FULL_HISTORY_PARTIAL_RECEIPT')
    fault = None
    assembly_ms = strict_span = items = None
    if finals:
        final = finals[0]
        receipt = final.get('receipt')
        candidate_items = receipt.get('items') if isinstance(receipt, dict) else None
        valid_start = (len(starts) == 1 and starts[0].get('case_id') == case
                       and starts[0].get('episode_count') == 151 and starts[0].get('carrier_span_present') is True
                       and isinstance(starts[0].get('source_episode'), str))
        valid_items = (isinstance(candidate_items, list) and len(candidate_items) <= LIMITS['budget']
                       and all(isinstance(item, dict) and isinstance(item.get('content'), str)
                               and isinstance(item.get('source_episode'), str) for item in candidate_items))
        if (len(finals) != 1 or final.get('case_id') != case or not valid_start or not valid_items
                or not finite_duration(final.get('elapsed_ms')) or not isinstance(receipt.get('stages'), list)
                or failures or partial or malformed_frame(meta, 'FULL_HISTORY_START', 'FULL_HISTORY_RECEIPT',
                                                          'FULL_HISTORY_FAILURE', 'FULL_HISTORY_PARTIAL_RECEIPT')):
            fault = 'invalid_receipt'
        else:
            items = candidate_items
            assembly_ms = final['elapsed_ms']
            strict_span = any(item['source_episode'] == starts[0]['source_episode']
                              and required_span in item['content'] for item in items)
    elif malformed_frame(meta, 'FULL_HISTORY_RECEIPT'):
        fault = 'invalid_receipt'
    elif len(failures) == 1 and failures[0].get('case_id') == case:
        fault = 'assembly_deadline' if failures[0].get('error') == 'Err(Elapsed(()))' else 'assembly_error'
    else:
        fault = 'missing_receipt'

    partial_ms = (partial[0].get('elapsed_ms') if len(partial) == 1 and partial[0].get('case_id') == case else None)
    if not finite_duration(partial_ms):
        partial_ms = None
    process_deadline_exceeded = (meta['external_timeout'] or not finite_duration(meta['elapsed_seconds'])
                                 or meta['elapsed_seconds'] > LIMITS['child_seconds'])
    if process_deadline_exceeded:
        fault = 'external_deadline'
    elif meta['process_fault'] is not None:
        fault = 'process_fault'
    elif assembly_ms is not None and assembly_ms > LIMITS['read_seconds'] * 1000:
        fault = 'assembly_deadline'
    elif items is not None and not ((meta['exit_code'] == 0 and strict_span)
                                  or (meta['exit_code'] == 101 and meta['strict_span_assertion'] and not strict_span)):
        fault = 'unexpected_process_exit'
    return {
        'receipt_observed': items is not None, 'assembly_elapsed_ms': assembly_ms, 'partial_elapsed_ms': partial_ms,
        'assembly_within_30': None if assembly_ms is None else assembly_ms <= LIMITS['read_seconds'] * 1000,
        'strict_original_source_span': strict_span, 'process_deadline_exceeded': process_deadline_exceeded,
        'runtime_completed': fault is None, 'retrieval_fault': fault,
        'delivered_items': items if fault is None else None,
    }


def compact_events(events, required_span):
    compact = []
    for event, data in events:
        body = dict(data)
        if event == 'FULL_HISTORY_QUERY' and isinstance(body.get('sql'), str):
            body['sql_sha256'] = hashlib.sha256(body.pop('sql').encode()).hexdigest()
        if event in ('FULL_HISTORY_RECEIPT', 'FULL_HISTORY_PARTIAL_RECEIPT'):
            receipt = body.get('receipt', body)
            if isinstance(receipt, dict) and isinstance(receipt.get('stages'), list):
                summarized = []
                for stage in receipt['stages']:
                    if not isinstance(stage, dict) or not isinstance(stage.get('items'), list):
                        summarized.append({'invalid_stage': True})
                        continue
                    items = []
                    for item in stage['items']:
                        if not isinstance(item, dict) or not isinstance(item.get('content'), str):
                            items.append({'invalid_stage_item': True})
                            continue
                        text = item['content']
                        payload = text.encode('utf-8', errors='surrogatepass')
                        items.append({'fact_id': item.get('fact_id'), 'source_episode': item.get('source_episode'),
                                      'content_bytes': len(payload), 'content_sha256': hashlib.sha256(payload).hexdigest(),
                                      'registered_span_present': required_span in text})
                    summarized.append({**stage, 'items': items})
                if receipt is body:
                    body['stages'] = summarized
                else:
                    body['receipt'] = {**receipt, 'stages': summarized}
        compact.append({'event': event, 'data': body})
    return compact


def preparation_step(journal, stage, summary, perform):
    record(journal, 'start', stage=stage)
    result = {'stage': stage, 'admitted': False}
    try:
        perform(result)
        result['admitted'] = True
    except Exception as error:
        result['fault'] = str(error)
        raise
    finally:
        save(OUT / summary, result)
        record(journal, 'terminal', **result)


def prepare():
    build, reads, original = preflight()
    with (OUT / 'preparation-attempts.jsonl').open('x', encoding='utf-8') as journal:
        def upgrade(result):
            RAW.mkdir(exist_ok=False)
            UPGRADED.parent.mkdir(exist_ok=False)
            shutil.copytree(SOURCE, UPGRADED)
            if hashes(UPGRADED) != original:
                raise RuntimeError('disposable upgrade input differs from retained snapshot')
            raw = RAW / 'upgrade.log'
            meta, events = run_child(argv(NEW, UPGRADE_TEST), child_environment(UPGRADED), raw, LIMITS['upgrade_seconds'])
            result.update(meta, original_snapshot_file_sha256=original, candidate_closed_file_sha256=hashes(UPGRADED))
            upgrades = bodies(events, 'FULL_HISTORY_UPGRADE')
            if (not passed_process(meta, LIMITS['upgrade_seconds']) or len(upgrades) != 1
                    or malformed_frame(meta, 'FULL_HISTORY_UPGRADE') or not finite_duration(upgrades[0].get('elapsed_ms'))
                    or not isinstance(upgrades[0].get('heat_counts'), list)):
                raise RuntimeError('upgrade observer/budget failed; no reads admitted')
            result['upgrade_receipt'] = upgrades[0]

        def audit(result):
            meta, events = invoke_clone(NEW, UPGRADED, AUDIT_TEST, BUILD_INPUT, RAW / 'audit.log', LIMITS['audit_seconds'])
            result.update(meta)
            inv = bodies(events, 'FULL_HISTORY_FORMATION')
            if not passed_process(meta, LIMITS['audit_seconds']) or len(inv) != 1 or malformed_frame(meta, 'FULL_HISTORY_FORMATION'):
                raise RuntimeError('post-upgrade source audit observer/budget failed')
            verifier = {'__name__': 'frozen_snapshot_verifier'}
            exec(compile(VERIFIER.read_bytes(), str(VERIFIER), 'exec'), verifier)
            save(OUT / 'post-upgrade-readback.json', inv[0])
            verification = verifier['verify_snapshot'](build, inv[0])
            upgraded = hashes(UPGRADED)
            original_unchanged = hashes(SOURCE) == original
            if not original_unchanged or upgraded != load(OUT / 'upgrade-execution.json')['candidate_closed_file_sha256']:
                raise RuntimeError('source or closed upgraded snapshot changed during isolated readback')
            save(OUT / 'snapshot-admission.json', {
                'source_verification': verification, 'source_segments': 151, 'source_snapshot_unchanged': original_unchanged,
                'upgraded_file_sha256': upgraded, 'audit_execution': meta,
                'historical_read_manifest_reference_sha256': reads['source_manifest_sha256'],
                'verified_build_registration_sha256': sha(BUILD_INPUT),
            })

        preparation_step(journal, 'upgrade', 'upgrade-execution.json', upgrade)
        preparation_step(journal, 'audit', 'audit-execution.json', audit)
    print('upgrade and source audit completed', flush=True)


def schedule(reads):
    return [dict(ordinal=index, case_id=case, arm=arm, child_seconds=LIMITS['child_seconds'],
                 command=argv(OLD if arm == 'legacy' else NEW, READ_TEST))
            for index, case in enumerate(reads['cases'])
            for arm in (('legacy', 'candidate') if index % 2 == 0 else ('candidate', 'legacy'))]


def capture():
    _, reads, original = preflight()
    admission = load(OUT / 'snapshot-admission.json')
    if (not admission['source_snapshot_unchanged'] or admission['source_segments'] != 151
            or admission['source_verification'] != {'complete': True, 'episode_count': 151}
            or hashes(UPGRADED) != admission['upgraded_file_sha256']):
        raise RuntimeError('candidate initial store or its source admission changed')
    plans = schedule(reads)
    save(OUT / 'schedule.json', {'scheduled': len(plans), 'paired_cases': len(reads['cases']), 'attempts': plans})
    for arm in ('legacy', 'candidate'):
        (OUT / arm).mkdir()
    all_rows, reader_rows = [], []
    with (OUT / 'attempts.jsonl').open('x', encoding='utf-8') as journal:
        for plan in plans:
            arm, case = plan['arm'], plan['case_id']
            registered = reads['cases'][case]
            raw = RAW / f"{plan['ordinal']}-{arm}-{case}.log"
            result = {**plan, 'raw_log': str(raw)}
            record(journal, 'start', **result)
            try:
                binary, snapshot = (OLD, SOURCE) if arm == 'legacy' else (NEW, UPGRADED)
                meta, events = invoke_clone(binary, snapshot, READ_TEST, READ_INPUT, raw, plan['child_seconds'], case)
                result.update(meta)
                outcome = classify_read(meta, events, case, registered['required_serialized_span'])
                result.update(outcome, events=compact_events(events, registered['required_serialized_span']))
            except Exception as error:
                result.update(runtime_completed=False, delivered_items=None, retrieval_fault='capture_fault', capture_fault=str(error))
                if raw.is_file():
                    result.update(raw_sha256=sha(raw), raw_bytes=raw.stat().st_size)
            save(OUT / arm / f'{case}.json', result)
            summary = {key: value for key, value in result.items() if key not in ('delivered_items', 'events')}
            record(journal, 'terminal', **summary)
            all_rows.append(summary)
            items = result['delivered_items'] or []
            if arm == 'candidate':
                reader_rows.append({'question_id': case, 'question_text': registered['query'], 'prompt_messages': [],
                                    'retrieval_fault': result['retrieval_fault'],
                                    'memory_context': [{'type': 'text', 'value': item['content']} for item in items]})
            print(case, arm, result.get('exit_code'), result.get('elapsed_seconds'), 'items', len(items), flush=True)
    with (OUT / 'reader-rows.jsonl').open('x', encoding='utf-8') as file:
        for row in reader_rows:
            file.write(json.dumps(row, ensure_ascii=True, allow_nan=False) + '\n')
        file.flush()
        os.fsync(file.fileno())
    source_unchanged = hashes(SOURCE) == original
    candidate_unchanged = hashes(UPGRADED) == admission['upgraded_file_sha256']
    save(OUT / 'reads-summary.json', {'scheduled': len(plans), 'paired_cases': len(reads['cases']), 'rows': all_rows,
                                    'source_snapshot_unchanged': source_unchanged,
                                    'candidate_initial_snapshot_unchanged': candidate_unchanged})
    if not source_unchanged or not candidate_unchanged:
        raise RuntimeError('initial snapshot integrity changed during capture; retain invalid evidence')


if __name__ == '__main__':
    import sys
    {'prepare': prepare, 'capture': capture}[sys.argv[1]]()
