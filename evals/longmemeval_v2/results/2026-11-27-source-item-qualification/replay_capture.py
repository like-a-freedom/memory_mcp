"""Reproduce both frozen reader reports using archived scorers; no inference."""
from collections import Counter
import hashlib
from importlib.machinery import SourceFileLoader
import importlib.util
import json
from pathlib import Path
import sys
sys.dont_write_bytecode = True

OUT = Path(__file__).resolve().parent
sys.path.insert(0,str(OUT.parents[3]))


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def require(condition,message):
    if not condition:
        raise ValueError(message)


def load(name,path):
    loader = SourceFileLoader(name,str(path))
    spec = importlib.util.spec_from_loader(name,loader)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    loader.exec_module(module)
    return module


def replay():
    acceptance = json.loads((OUT/'acceptance.json').read_text())
    decision = json.loads((OUT/'candidate-decision-v1.json').read_text())
    require(sha(OUT/'acceptance.json')==decision['acceptance_sha256'],'acceptance hash')
    for path,identity in decision['artifact_hashes'].items():
        require(sha(OUT/path)==identity,'artifact hash '+path)
    require(sha(OUT/'fresh16/source-review.json')==acceptance['source_review_sha256'],
            'pre-inference source review hash')
    require(sha(OUT/'historical-range-outputs.json')==acceptance['historical_baseline']['sha256'],
            'historical baseline outputs hash')
    import evals.longmemeval_v2
    package = evals.longmemeval_v2
    # source_references binds the matcher at import time: archive dependency first.
    matcher = load(package.__name__+'.exact_answer_quality',OUT/'exact_answer_quality.py.source')
    package.exact_answer_quality = matcher
    reference = load(package.__name__+'.source_references',OUT/'source_references.py.source')
    package.source_references = reference
    reader = load(package.__name__+'.captured_reader',OUT/'reader_protocol.py.source')
    trials,requests,strict,values,binding,proof = 0,0,0,0,0,0
    summaries = {}
    for cohort in ('shared40','fresh16'):
        path = OUT/cohort
        plan = json.loads((path/'registration.json').read_text())
        require(sha(path/'registration.json')==acceptance['registrations'][cohort],'registration')
        for key,name in [('runner_sha256','reader_protocol.py'),
                         ('source_reference_sha256','source_references.py'),
                         ('answer_matcher_sha256','exact_answer_quality.py'),
                         ('attempt_recorder_sha256','inference_attempts.py')]:
            require(sha(OUT/(name+'.source'))==plan[key],'archived implementation')
        # Registration retains original machine paths; archived bytes are portable.
        rows_path,truth_path = path/'rows.jsonl',path/'truth.json'
        require(sha(rows_path)==plan['rows_sha256'] and sha(truth_path)==plan['truth_sha256'],'input hashes')
        rows = reader.read_rows(rows_path)
        truth = json.loads(truth_path.read_text())['cases']
        outputs = json.loads((path/'outputs.json').read_text())
        report = json.loads((path/'report.json').read_text())
        prompts = {r['question_id']:{v:reader.protocol_messages(r,v)for v in plan['variants']}for r in rows}
        require(prompts==json.loads((path/'prompts.json').read_text()),'prompt replay')
        for variant in plan['variants']:
            result = reader.summarize(rows,truth,outputs[variant],variant)
            require(result==report['variants'][variant],'report replay '+cohort+variant)
            summaries[cohort,variant] = result
            trials += result['selected']
            if variant == reference.ITEM_VARIANT:
                strict += result['completed_correct']
                values += result['value_correct']
                binding += result['source_bound']
                proof += result['proof_covered']
        events = [json.loads(line)for line in(path/'inference-events.jsonl').read_text().splitlines()]
        starts = [e for e in events if e['event']=='attempt_started']
        ends = [e for e in events if e['event']=='attempt_finished']
        attempts = [json.loads(line)for line in(path/'inference-attempts.jsonl').read_text().splitlines()]
        require(Counter(e['request_id']for e in starts)==Counter(e['request_id']for e in ends)
                ==Counter(e['request_id']for e in attempts),'request accounting')
        require(len({e['request_id']for e in starts})==len(starts)
                and all(e['attempt']==1 for e in starts),'no retries/duplicates')
        requests += len(starts)
        if cohort == 'shared40':
            historical = json.loads((OUT/'historical-range-outputs.json').read_text())
            summaries['historical',reference.VARIANT] = reader.summarize(
                rows,truth,historical[reference.VARIANT],reference.VARIANT)
    regressions,gains = {},{}
    for cohort,baseline in [('shared40',summaries['historical',reference.VARIANT]),
                            ('fresh16',summaries['fresh16',reference.VARIANT])]:
        current = summaries[cohort,reference.ITEM_VARIANT]['cases']
        regressions[cohort] = [q for q,c in current.items()
                               if baseline['cases'][q]['value_correct'] and not c['value_correct']]
        gains[cohort] = [q for q,c in current.items()
                         if not baseline['cases'][q]['value_correct'] and c['value_correct']]
    require(regressions==decision['value_regressions'] and gains==decision['value_gains'],
            'paired value changes')
    require(trials==requests==72 and strict==decision['whole_item_strict_correct'],'fixed denominators')
    require(values==decision['whole_item_value_correct']
            and binding==decision['whole_item_source_bound']
            and proof==decision['whole_item_proof_covered'],'decision metric totals')
    return {'prompts_and_case_grades_reproduced':trials,'requests_started_and_terminal':requests,
            'whole_item_strict':strict,'whole_item_value':values,'whole_item_binding':binding,
            'whole_item_proof':proof,'whole_item_denominator':56,'inference_calls':0}


if __name__ == '__main__':
    print(json.dumps(replay(),indent=2))
