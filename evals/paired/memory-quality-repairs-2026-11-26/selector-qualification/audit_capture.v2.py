"""Supplementary both-arm audit; Rust remains the authoritative observation scorer."""
import argparse
import json
from pathlib import Path
def require(condition, message):
    if not condition:
        raise ValueError(message)


FIELDS = {'fact_types', 'as_of', 'valid_at', 'known_at', 'known_revision', 'budget', 'view_mode', 'window_start', 'window_end', 'compact'}

def fingerprint(value):
    import hashlib
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()

def key(row):
    return json.dumps(row['key'], sort_keys=True)

def audit(tasks, capture, traces):
    expected = {key(task): task for task in tasks['tasks']}
    if not expected or len(expected) != len(tasks['tasks']):
        return {'schema': 'temporal-both-arm-audit/v2', 'eligible': False, 'scheduled_arm_trials': len(tasks['tasks']) * 2, 'passing_arm_trials': 0, 'error': 'empty or duplicate task identity'}
    raw = {}
    for path in traces.glob('*.json'):
        trace = json.loads(path.read_text())
        if trace.get('identity', {}).get('phase') == 'recall':
            identity = (trace['identity']['arm'], key(trace['identity']))
            raw.setdefault(identity, []).append(trace)
    rows = []
    missing = []
    for arm in ('incumbent', 'candidate'):
        artifact_path = capture / ('independent-' + arm + '.json')
        if not artifact_path.exists():
            missing.append(arm)
            rows.extend(({'arm': arm, 'key': task['key'], 'passed': False, 'error': 'missing arm artifact'} for task in expected.values()))
            continue
        artifact = json.loads(artifact_path.read_text())
        observed = artifact['agent_observed']
        receipts = observed['receipts']
        probes = artifact['independent_memory']
        coverage = len(receipts) == len(expected) == len(probes) and {key(r) for r in receipts} == set(expected) and ({key(r) for r in probes} == set(expected))
        for identity, task in expected.items():
            row = {'arm': arm, 'key': task['key'], 'passed': False}
            try:
                receipt = next((r for r in receipts if key(r) == identity))
                probe = next((r for r in probes if key(r) == identity))
                candidates = raw.get((arm, identity), [])
                require(coverage and observed['arm'] == arm, 'arm coverage mismatch')
                require(receipt['status'] == 'completed' and receipt['task_oracle_passed'] is True, 'task oracle failure')
                require(receipt['direct_memory_success'] is True and probe['score']['evidence_success'] is True, 'memory outcome failure')
                require(receipt['hard_violations'] == [] and receipt['tool_calls'] == 1, 'violation or call-budget failure')
                require(receipt['read_capture_status'] == 'observed' and len(receipt['read_diagnostics']) == 1, 'missing unique observed read')
                read = receipt['read_diagnostics'][0]
                require(read['outcome'] in ('empty', 'nonempty') and read.get('source_coverage') is True, 'unsuccessful evidence read')
                matches = read.get('selector_argument_matches', {})
                require(read.get('selector_expectation_registered') is True and set(matches) == FIELDS and all((v is True for v in matches.values())), 'selector contract mismatch')
                expectation = task['oracle']['read_expectation']
                if expectation.get('valid_at'):
                    require(read.get('requested_valid_at_match') is True and read.get('returned_valid_at_match') is True, 'valid-time mismatch')
                    if receipt.get('known_revision_checkpoint') is not None:
                        require(read.get('requested_cutoff_match') is True and read.get('returned_cutoff_match') is True, 'checkpoint cutoff mismatch')
                require(len(candidates) == 1, 'missing or duplicate raw trace')
                trace = candidates[0]
                require(trace['identity']['product_binary_sha256'] == observed['identity']['product']['binary_sha256'], 'trace product mismatch')
                require(len(trace['calls']) == 1, 'raw call count mismatch')
                call = trace['calls'][0]
                require(call['tool'] == 'assemble_context' and call['call_failed'] is False, 'raw read failed')
                arguments = call['arguments']
                require(isinstance(arguments.get('query'), str) and arguments['query'].strip(), 'blank actual model query')
                require(fingerprint(arguments) == call['arguments_sha256'] and fingerprint(call['response']) == call['response_sha256'], 'raw trace digest mismatch')
                require(fingerprint(arguments['query']) == read['request_query_sha256'], 'query detached from scored read')
                require(fingerprint({k: v for k, v in arguments.items() if k != 'query'}) == read['non_query_arguments_sha256'], 'selectors detached from scored read')
                row['passed'] = True
                row['actual_query'] = arguments['query']
                row['actual_selectors'] = {k: v for k, v in arguments.items() if k != 'query'}
            except (ValueError, KeyError, StopIteration, TypeError) as error:
                row['error'] = str(error) or type(error).__name__
            rows.append(row)
    passing = sum((row['passed'] for row in rows))
    scheduled = len(expected) * 2
    return {'schema': 'temporal-both-arm-audit/v2', 'eligible': passing == scheduled and len(rows) == scheduled, 'scheduled_arm_trials': scheduled, 'passing_arm_trials': passing, 'missing_arms': missing, 'rows': rows}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tasks', type=Path, required=True)
    parser.add_argument('--capture', type=Path, required=True)
    parser.add_argument('--traces', type=Path, required=True)
    args = parser.parse_args()
    report = audit(json.loads(args.tasks.read_text()), args.capture, args.traces)
    print(json.dumps(report, indent=2))
    return 0 if report['eligible'] else 1
if __name__ == '__main__':
    raise SystemExit(main())
