"""Replay the fixed v3 grades and linked manual review without inference."""
from collections import Counter
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
sys.dont_write_bytecode = True

OUT = Path(__file__).resolve().parent
sys.path.insert(0,str(OUT.parents[2]))


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def load(name,path):
    spec = importlib.util.spec_from_file_location(name,path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def replay():
    plan_path = OUT/'registered/frozen-plan.json'
    plan = json.loads(plan_path.read_text())
    acceptance = json.loads((OUT/'acceptance.json').read_text())
    decision = json.loads((OUT/'decision.json').read_text())
    integrity = json.loads((OUT/'capture-integrity-v1.json').read_text())
    require(sha(OUT/'decision.json')==integrity['decision_sha256'],'decision hash')
    for name,identity in integrity['request_accounting_hashes'].items():
        require(sha(OUT/'registered/capture'/name)==identity,'request accounting hash '+name)
    require(sha(plan_path)==acceptance['frozen_plan_sha256'],'registered plan hash')
    for key,name in [('source_sha256','runner.source.py'),
                     ('source_action_scorer_sha256','source-actions.source.py'),
                     ('mcp_binding_sha256','mcp-binding.source.py'),
                     ('attempt_observer_sha256','inference_attempts.source.py')]:
        require(sha(OUT/name)==plan[key],'archived implementation '+key)
    import evals.paired
    scorer = load('evals.paired.skill_source_actions',OUT/'source-actions.source.py')
    evals.paired.skill_source_actions = scorer
    runner = load('captured_source_action_workflow',OUT/'runner.source.py')
    review_path = OUT/'source-review.json'
    require(sha(review_path)==decision['review_sha256'],'manual review hash')
    reviews = json.loads(review_path.read_text())['rows']
    indexed = {r['identity']:r for r in reviews}
    identities = {f"{c['skill']}-{c['id']}-{a}"for c in plan['cases']for a in plan['arms']}
    require(len(reviews)==len(indexed)==12 and set(indexed)==identities,'review denominator')
    full,operations = 0,0
    for case in plan['cases']:
        for arm in plan['arms']:
            identity = f"{case['skill']}-{case['id']}-{arm}"
            path = OUT/'registered/capture'/identity/'trial.json'
            row = json.loads(path.read_text())
            reviewed = indexed[identity]
            require(sha(path)==reviewed['trace_sha256'],'trial hash '+identity)
            grade = runner.grade_observed(case,row['result'],row['trace'])
            require(grade==row['grade'],'captured operation grade '+identity)
            expected = bool(reviewed['semantic_pass'] and reviewed['within_word_budget']
                            and grade['operation_and_exact_arguments'] and row['terminal']=='answered'
                            and not grade['unnecessary_reference_loading'])
            require(expected==reviewed['full_pass'],'linked full grade '+identity)
            full += expected
            operations += grade['operation_and_exact_arguments']
    events = [json.loads(line)for line in (OUT/'registered/capture/inference-events.jsonl').read_text().splitlines()]
    starts = [e for e in events if e['event']=='attempt_started']
    ends = [e for e in events if e['event']=='attempt_finished']
    attempts = [json.loads(line)for line in (OUT/'registered/capture/inference-attempts.jsonl').read_text().splitlines()]
    require(Counter(e['request_id']for e in starts)==Counter(e['request_id']for e in ends)
            ==Counter(e['request_id']for e in attempts),'request accounting')
    require(len({e['request_id']for e in starts})==len(starts)
            and all(e['attempt']==1 for e in starts),'request uniqueness and zero retries')
    require(len(starts)==decision['requests']['started']
            and len(ends)==len(attempts)==decision['requests']['terminal']
            and dict(Counter(e['outcome']for e in attempts))==decision['requests']['outcomes']
            and all(e['terminal'] is True for e in attempts)
            and {e['question_id']for e in starts}==identities,'request accounting fixed totals/outcomes')
    require(full==decision['full_passes'] and operations==decision['operation_passes_frozen_v3'],'decision totals')
    return {'trials_reproduced':12,'operation_passes':operations,'full_passes':full,
            'requests_started_and_terminal':len(starts),'inference_calls':0,
            'review':'linked primary manual judgments, not independent re-adjudication'}


if __name__ == '__main__':
    print(json.dumps(replay(),indent=2))
