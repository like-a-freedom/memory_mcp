"""Replay saved operation grades, prompts, review linkage and request accounting.

No inference, native binary, database or model server is used. Human semantic
judgments are verified as linked inputs, not automatically re-adjudicated.
"""
from collections import Counter
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
sys.dont_write_bytecode = True

ROOT = Path(__file__).resolve().parent
sys.path.insert(0,str(ROOT.parents[2]))

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def require(condition, message):
    if not condition:
        raise ValueError(message)


def replay():
    acceptance = json.loads((ROOT/'acceptance.json').read_text())
    decision = json.loads((ROOT/'candidate-decision-v1.json').read_text())
    integrity = json.loads((ROOT/'capture-integrity-v1.json').read_text())
    request_integrity = json.loads((ROOT/'request-integrity-v2.json').read_text())
    require(sha(ROOT/'capture-integrity-v1.json')==request_integrity['previous_integrity_sha256'],
            'previous integrity hash')
    require(sha(ROOT/'candidate-decision-v1.json')==request_integrity['decision_sha256'],'decision hash')
    require(sha(ROOT/'acceptance.json')==decision['acceptance_sha256'],'acceptance hash')
    spec = importlib.util.spec_from_file_location('captured_workflow',ROOT/'runner.source.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    full,operations,scheduled,prompts,requests = 0,0,0,0,0
    for registration in acceptance['plans']:
        folder = ROOT/registration['directory']
        for name,identity in request_integrity['cohorts'][registration['directory']].items():
            require(sha(folder/'capture'/name)==identity,'request accounting hash '+name)
        plan_path = folder/'frozen-plan.json'
        plan = json.loads(plan_path.read_text())
        require(sha(plan_path)==registration['sha256'],'plan hash')
        require(sha(ROOT/'runner.source.py')==plan['source_sha256'],'captured runner hash')
        require(sha(ROOT/'mcp-binding.source.py')==plan['mcp_binding_sha256'],'captured binding hash')
        review_path = folder/'semantic-review-v1.json'
        require(sha(review_path)==integrity['cohorts'][registration['directory']]['review_sha256'],
                'semantic review hash '+registration['directory'])
        review = json.loads(review_path.read_text())
        require(review['frozen_plan_sha256']==registration['sha256'],'review plan hash')
        rows = {r['identity']:r for r in review['rows']}
        identities = {f"{c['skill']}-{c['id']}-{a}"for c in plan['cases']for a in plan['arms']}
        require(len(rows)==len(review['rows'])==registration['scheduled'] and set(rows)==identities,'review scheduled identities')
        for case in plan['cases']:
            for arm in plan['arms']:
                identity = f"{case['skill']}-{case['id']}-{arm}"
                trial_path = folder/'capture'/identity/'trial.json'
                trial = json.loads(trial_path.read_text())
                reviewed = rows[identity]
                require(sha(trial_path)==reviewed['trial_sha256'],'review trial hash '+identity)
                grade = module.grade_observed(case,trial['result'],trial['trace'])
                require(grade==trial['grade'],'operation replay '+identity)
                require(grade['operation_and_exact_arguments']==reviewed['registered_operation_pass'],'operation review '+identity)
                require(reviewed['full_pass']==bool(reviewed['semantic_clauses'] and grade['operation_and_exact_arguments']
                        and trial['terminal']=='answered' and not grade['unnecessary_reference_loading']),'full review '+identity)
                schema = None
                if case['skill']=='memory-mcp':
                    receipts = [json.loads(line)for line in(trial_path.parent/'mcp.jsonl').read_text().splitlines()]
                    tools = next(row['response']['result']['tools']for row in receipts if row['request']['method']=='tools/list')
                    schema = {module.MCP_HOST+t['name']:t['inputSchema']for t in tools}
                expected = module.create_messages(case,plan['skills'][case['skill']][arm]['SKILL.md'],schema,
                                                  workflow_protocol=plan.get('workflow_protocol','legacy'))
                require(expected==trial['result']['turns'][0]['request_messages'],'prompt replay '+identity)
                full += reviewed['full_pass']
                operations += grade['operation_and_exact_arguments']
                scheduled += 1
                prompts += 1
        events = [json.loads(line)for line in(folder/'capture/inference-events.jsonl').read_text().splitlines()]
        attempts = [json.loads(line)for line in(folder/'capture/inference-attempts.jsonl').read_text().splitlines()]
        starts = [e['request_id']for e in events if e['event']=='attempt_started']
        ends = [e['request_id']for e in events if e['event']=='attempt_finished']
        require(Counter(starts)==Counter(ends)==Counter(a['request_id']for a in attempts),'request terminals')
        require(len(set(starts))==len(starts) and all(a['attempt']==1 for a in attempts),'request uniqueness/retry')
        expected_accounting = integrity['cohorts'][registration['directory']]
        expected_outcomes = decision['cohorts'][registration['directory']]['inference_attempt_outcomes']
        require(len(starts)==expected_accounting['request_starts']
                and len(ends)==len(attempts)==expected_accounting['request_terminals']
                and dict(Counter(a['outcome']for a in attempts))==expected_outcomes
                and all(a['terminal'] is True for a in attempts)
                and {a['question_id']for a in attempts}==identities,'request accounting fixed totals/outcomes')
        requests += len(starts)
    require(scheduled==decision['scheduled'] and full==decision['strict_full_pass'] and operations==decision['operation_pass'],'decision totals')
    return {'scheduled_trials':scheduled,'operation_grades_reproduced':scheduled,'initial_prompts_reproduced':prompts,
            'strict_reviewed_pass':full,'operation_pass':operations,'request_starts_and_terminals':requests,
            'abandoned_requests':0,'inference_calls':0,'semantic_review_scope':'pinned primary non-blinded manual judgments, not new independent adjudication'}

if __name__=='__main__':
    print(json.dumps(replay(),indent=2))
