"""Offline, versioned operation-trace regrade for the frozen skill capture."""

import argparse
import hashlib
import json
from pathlib import Path
import runpy

ARCHIVE = {
    'source_sha256': 'v3runner.source.py',
    'attempt_observer_sha256': 'inference_attempts.source.py',
    'mcp_binding_sha256': 'mcp_binding.source.py',
}


def comparison_trace(trace):
    result = []
    for row in trace:
        request = row['request']
        args = request.get('arguments')
        if (request.get('action') == 'call' and request.get('name') == 'mcp__memory__extract'
                and isinstance(args, dict) and args.get('include_fact_evidence') is True):
            comparable = {key: value for key, value in args.items() if key != 'include_fact_evidence'}
            row = {**row, 'request': {**request, 'arguments': comparable}}
        result.append(row)
    return result


def operation_observed(row):
    request = row['request']
    response = row['response']
    if not isinstance(response, dict) or response.get('denied') or response.get('host_error'):
        return False
    if request.get('action') == 'call':
        return True
    argv = request.get('argv')
    return request.get('action') == 'run' and bool(argv) and argv[-1] not in {'--help', '-h'}


def regrade_trial(case, trial, score):
    terminal = trial['result']['terminal']
    trace = trial['trace']
    # The archived scorer gates every dimension on the final answer. Bypass only
    # that gate for an observed operation, then preserve the trial state separately.
    operation_view = {'terminal': 'answered'} if any(map(operation_observed, trace)) else trial['result']
    grade = score(case, operation_view, comparison_trace(trace))
    could_pass = (terminal == 'answered' and grade['operation_and_exact_arguments']
                  and grade['source_binding_observed'] is not False
                  and not grade['unnecessary_reference_loading'])
    return {'terminal': terminal, 'grade': grade, 'full_row_pass': None if could_pass else False}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def regrade_capture(plan, capture, score):
    summary_bytes = (capture / 'summary.json').read_bytes()
    summary = json.loads(summary_bytes)
    schedule = {f"{case['skill']}-{case['id']}-{arm}": (case, arm)
                for case in plan['cases'] for arm in plan['arms']}
    if (summary['scheduled'] != len(schedule) or len(summary['rows']) != len(schedule)
            or {row['identity'] for row in summary['rows']} != schedule.keys()):
        raise ValueError('offline regrade requires the complete scheduled capture')

    rows = []
    for original in summary['rows']:
        identity = original['identity']
        case, arm = schedule[identity]
        raw = (capture / identity / 'trial.json').read_bytes()
        trial = json.loads(raw)
        if (original['arm'] != arm or original['skill'] != case['skill'] or original['id'] != case['id']
                or any(trial.get(key) != value for key, value in original.items())
                or trial['terminal'] != trial['result']['terminal']):
            raise ValueError(f'{identity} trial differs from frozen schedule or capture summary')
        try:
            current = regrade_trial(case, trial, score)
        except Exception as error:
            current = {'terminal': original['terminal'], 'full_row_pass': False,
                       'grade': {'operation_and_exact_arguments': False},
                       'regrade_error': {'type': type(error).__name__, 'message': str(error)}}
        previous_grade = original['grade']
        grade = current['grade']
        difference = {key: {'original': previous_grade.get(key), 'regraded': grade.get(key)}
                      for key in previous_grade.keys() | grade.keys()
                      if previous_grade.get(key) != grade.get(key)}
        rows.append({**current, 'identity': identity, 'arm': arm, 'skill': case['skill'], 'id': case['id'],
                     'binding': original['binding'], 'original_trial_sha256': sha256(raw),
                     'original_grade': previous_grade, 'grade_difference': difference})
    return {'scheduled': len(schedule), 'capture_summary_sha256': sha256(summary_bytes),
            'original_operation_passes': sum(r['original_grade']['operation_and_exact_arguments'] for r in rows),
            'regraded_operation_passes': sum(r['grade']['operation_and_exact_arguments'] for r in rows),
            'rows_with_differences': sum(bool(row['grade_difference']) for row in rows), 'rows': rows}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run_directory', type=Path)
    directory = parser.parse_args().run_directory
    plan_bytes = (directory / 'frozen-plan.json').read_bytes()
    plan = json.loads(plan_bytes)
    for digest_key, filename in ARCHIVE.items():
        if sha256((directory / filename).read_bytes()) != plan[digest_key]:
            raise ValueError(f'archived bytes differ from the frozen {digest_key}')
    score = runpy.run_path(str(directory / ARCHIVE['source_sha256']))['grade_observed']
    result = regrade_capture(plan, directory / 'capture', score)
    report = {
        **result, 'schema': 'memory-mcp-skill-operation-regrade/v1',
        'frozen_plan_sha256': sha256(plan_bytes), 'original_grader_sha256': plan['source_sha256'],
        'regrade_source_sha256': sha256(Path(__file__).read_bytes()),
        'corrections': ['Score observed operations independently of final answer state.',
                        'Ignore only explicit MCP extract include_fact_evidence=true in argument comparison.'],
        'full_row_pass': 'false is a known failure; null requires separate semantic adjudication',
    }
    with (directory / 'operation-regrade-v1.json').open('x', encoding='utf-8') as output:
        json.dump(report, output, indent=2, ensure_ascii=False, sort_keys=True)
        output.write('\n')


if __name__ == '__main__':
    main()
