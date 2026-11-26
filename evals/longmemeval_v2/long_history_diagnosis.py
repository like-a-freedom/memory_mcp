"""Diagnose archived long-history reads without model calls or inferred stages.

Truth is a separately reviewed source contract, never a reader/scorer output.
Exact phrase-set matching is a diagnostic contract, not a semantic abstention
judge. Missing intermediate receipts cannot localize formation or selection.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import statistics
import sys

from .exact_answer_quality import exact_phrase_set_match
from .reviewed_span_coverage import decode_saved_text, read_jsonl
from .text_cohort import verified_span


def diagnose(cohort, trajectories, truth, prompts, readers):
    if truth.get('schema') != 'memory-mcp-long-history-truth/v1':
        raise ValueError('unsupported independent truth schema')
    if cohort.get('schema') != 'memory-mcp-longmemeval-v2-text-cohort/v1':
        raise ValueError('unsupported cohort schema')

    def indexed(rows, key):
        result = {}
        for row in rows:
            case_id = row.get(key)
            if not isinstance(case_id, str) or case_id in result:
                raise ValueError('missing or duplicate case identity')
            result[case_id] = row
        return result

    sources = indexed(trajectories, 'id')
    prompt_rows = indexed(prompts, 'question_id')
    reader_rows = indexed(readers, 'question_id')
    ids = set(cohort['reviews'])
    if set(truth['cases']) != ids or set(prompt_rows) != ids or not set(reader_rows) <= ids:
        raise ValueError('cohort, truth, prompt or reader identities do not match')
    cases = {}
    latencies = []
    for case_id, review in cohort['reviews'].items():
        span = verified_span(sources[review['trajectory_id']], review)
        label = truth['cases'][case_id]
        if label['reviewed_span_sha256'] != review['span_sha256']:
            raise ValueError('truth source digest does not match reviewed source')
        markers = label['support_markers']
        if not markers or any(not isinstance(marker, str) or not marker or marker not in span for marker in markers):
            raise ValueError('truth support markers must occur in reviewed source')
        accepted = label['accepted_answers']
        if not accepted or any(not isinstance(answer, str) or not answer.strip() for answer in accepted):
            raise ValueError('truth requires nonempty independent accepted answers')
        prompt = prompt_rows[case_id]
        context = prompt.get('memory_context')
        if not isinstance(context, list):
            raise ValueError('missing memory context is not an empty observed context')
        text = '\n'.join(decode_saved_text(item['value']) for item in context
                         if item.get('type') == 'text' and isinstance(item.get('value'), str))
        complete = any(span in decode_saved_text(item['value']) for item in context
                       if item.get('type') == 'text' and isinstance(item.get('value'), str))
        marker_presence = {marker: marker in text for marker in markers}
        reader = reader_rows.get(case_id)
        if reader and 'memory_context_sha256' in reader:
            context_digest = hashlib.sha256(json.dumps(context, ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
            if reader['memory_context_sha256'] != context_digest:
                raise ValueError('reader context digest mismatch')
        answer = reader.get('response_parsed_boxed') if reader else None
        verdict = ('unknown_missing_reader' if not isinstance(answer, str) else
                   'correct' if any(exact_phrase_set_match(answer, expected) for expected in accepted) else 'incorrect')
        localization = ('reader_use_with_complete_reviewed_evidence'
                        if complete and reader and 'memory_context_sha256' in reader and verdict == 'incorrect' else
                        'end_to_end_failure_stage_unknown' if verdict == 'incorrect' else
                        'no_failure_under_registered_answer_contract' if verdict == 'correct' else
                        'unknown_missing_reader')
        latency = prompt.get('memory_query_duration_seconds')
        if latency is not None:
            if isinstance(latency, bool) or not isinstance(latency, (int, float)) or not math.isfinite(latency) or latency < 0:
                raise ValueError('query latency must be finite and nonnegative')
            latencies.append(latency)
        cases[case_id] = {
            'reviewed_span_sha256': review['span_sha256'],
            'source_input': 'verified_reviewed_span',
            'formation': 'unknown_no_intermediate_receipt',
            'evidence_selection': 'unknown_no_intermediate_receipt',
            'packaging': 'unknown_no_prepackaging_receipt',
            'delivered_complete_reviewed_span': complete,
            'delivered_support_markers': marker_presence,
            'context_nonempty': bool(text.strip()),
            'context_tokens': prompt.get('memory_context_token_count'),
            'context_was_truncated': prompt.get('memory_context_was_truncated'),
            'answer_kind': label['answer_kind'],
            'reader_context_link': 'verified_context_digest' if reader and 'memory_context_sha256' in reader else 'unknown_missing_context_digest',
            'reader_answer': answer,
            'reader_verdict': verdict,
            'failure_localization': localization,
            'query_seconds': latency,
        }
    return {
        'schema': 'memory-mcp-long-history-diagnosis/v1',
        'answer_contract': 'strict normalized phrase-set equality against independently reviewed accepted answers; unlisted valid paraphrases may be false negatives',
        'stage_contract': 'full-span absence is conservative; marker presence alone never establishes semantic sufficiency; missing intermediate receipts stay unknown',
        'selected': len(cases),
        'correct': sum(case['reader_verdict'] == 'correct' for case in cases.values()),
        'complete_reviewed_spans': sum(case['delivered_complete_reviewed_span'] for case in cases.values()),
        'reader_failures_with_complete_evidence': sum(case['failure_localization'] == 'reader_use_with_complete_reviewed_evidence' for case in cases.values()),
        'latency_seconds': {'observed': len(latencies), 'median': statistics.median(latencies) if latencies else None, 'max': max(latencies) if latencies else None},
        'cases': cases,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('cohort', 'trajectories', 'truth', 'prompts', 'readers'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    paths = {name: getattr(args, name) for name in ('cohort', 'trajectories', 'truth', 'prompts', 'readers')}
    report = diagnose(*[read_jsonl(path) if name in ('prompts', 'readers') else json.loads(path.read_text()) for name, path in paths.items()])
    report['input_sha256'] = {name: hashlib.sha256(path.read_bytes()).hexdigest() for name, path in paths.items()}
    report['input_paths'] = {name: str(path) for name, path in paths.items()}
    rendered = json.dumps(report, indent=2, ensure_ascii=False) + '\n'
    if args.output:
        args.output.write_text(rendered)
    else:
        sys.stdout.write(rendered)


if __name__ == '__main__':
    main()
