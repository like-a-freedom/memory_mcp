"""Describe the saved finite capture; never dispatch inference or regrade answers."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def summarize():
    capture = ROOT / 'registered/capture'
    requests = capture / 'inference-attempts.jsonl'
    attempts = [json.loads(line) for line in requests.read_text().splitlines()]
    trials = [json.loads(path.read_text()) for path in sorted(capture.glob('*/trial.json'))]
    preflight_path = ROOT / 'runtime-preflight.json'
    preflight = json.loads(preflight_path.read_text())
    arms = {}
    for arm in ('previous', 'current'):
        selected = [row for row in attempts if row['question_id'].endswith('-' + arm)]
        selected_trials = [row for row in trials if row['arm'] == arm]
        arms[arm] = {
            'trials': len(selected_trials),
            'requests': len(selected),
            'reported_prompt_tokens': sum(row['usage']['prompt_tokens'] for row in selected),
            'reported_completion_tokens': sum(row['usage']['completion_tokens'] for row in selected),
            'reported_cached_prompt_tokens': sum(
                (row['usage'].get('prompt_tokens_details') or {}).get('cached_tokens', 0) or 0
                for row in selected),
            'observed_request_seconds_sum': sum(row['elapsed_seconds'] for row in selected),
            'observed_trial_seconds_sum': sum(row['elapsed_seconds'] for row in selected_trials),
            'maximum_request_seconds': max(row['elapsed_seconds'] for row in selected),
            'maximum_trial_seconds': max(row['elapsed_seconds'] for row in selected_trials),
        }
    return {
        'schema': 'source-action-v4-descriptive-cost/v1',
        'attempts_sha256': hashlib.sha256(requests.read_bytes()).hexdigest(),
        'by_arm': arms,
        'request_scope': '54 study requests plus one separately retained startup request; health probes are not inference',
        'study_inference_requests': len(attempts),
        'startup_inference_requests': 1,
        'total_observed_inference_requests': len(attempts) + 1,
        'startup_preflight_sha256': hashlib.sha256(preflight_path.read_bytes()).hexdigest(),
        'startup_preflight_seconds': preflight['elapsed_seconds'],
        'startup_reported_usage': preflight['response']['usage'],
        'trials_exceeding_prospective_20_second_whole_trial_assumption':
            sum(row['elapsed_seconds'] > 20 for row in trials),
        'scheduled_trials': len(trials),
        'illustrative_24_family_pilot_bound_at_current_caps_seconds': 24 * 2 * (180 + 30),
        'bound_scope': 'current180s whole-trial cap plus prospective30s formation; excludes startup/export; not an activated C2 registration or predicted cost',
        'interpretation': [
            'Request20s and whole-trial180s are different budgets. All12 observed trials exceed20s.',
            'Do not use the prospective20s whole-trial assumption as measured feasibility.',
            'Recorded trial-minus-request time includes tool/host/other gaps; it is not a measured storage stage.',
            'Prefix-cache usage differs; bundled treatment and fixed execution order preclude a causal speed claim.',
            'Reported prompt tokens include repeated cached context; no billing, engine-memory or uncached-token-cost inference.',
            'No model calls, no changes to acceptance or historical outputs.',
        ],
    }


if __name__ == '__main__':
    print(json.dumps(summarize(), indent=2))
