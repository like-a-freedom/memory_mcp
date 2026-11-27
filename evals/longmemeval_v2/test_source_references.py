"""Public reader contract controls with independently worked source evidence."""
import hashlib
import json
import re
from pathlib import Path
import unittest
from tempfile import TemporaryDirectory

from evals.longmemeval_v2.reader_protocol import classify, protocol_messages, freeze

VARIANT = 'source-range-final-first'
ITEM_VARIANT = 'source-item-raw-final-first'


def item_handles(row):
    return re.findall(r'^SOURCE ([a-z0-9-]+) ',protocol_messages(row,ITEM_VARIANT)[-1]['content'],re.MULTILINE)


class SourceReferenceTests(unittest.TestCase):
    def setUp(self):
        self.raw = 'Retention: 42 days.\nUnrelated: 99 days.\n'
        self.sha = hashlib.sha256(self.raw.encode()).hexdigest()
        self.row = {'question_id':'numeric', 'question_text':'What is the retention period?',
                    'prompt_messages':[], 'memory_context':[{'type':'text','value':self.raw}]}
        self.truth = {'answer_kind':'single_value','accepted_answers':['42 days'],
                      'evidence_alternatives':[[{'source_item_index':0,'raw_sha256':self.sha,
                                                 'raw_range':[0,19]}]]}

    def test_raw_item_protocol_delivers_source_without_an_interpretation_graph(self):
        self.row['memory_context'] = [{'type':'text','value':
            "[1] navigation 'Menu'\n  [2] link 'Archive'\n"}]
        try:
            messages = protocol_messages(self.row,'source-item-raw-final-first')
        except ValueError as error:
            self.fail(f'raw-only source delivery must be registered: {error}')

        self.assertEqual([message['role'] for message in messages],['system','user'])
        self.assertEqual(messages[-1]['content'].split('):\n',1)[1],
            "[1] navigation 'Menu'\n  [2] link 'Archive'\n")

    def test_whole_item_selects_existing_text_without_copying_hashes_or_line_ranges(self):
        messages = protocol_messages(self.row,ITEM_VARIANT)
        self.assertIn('SOURCE item-0',messages[-1]['content'])
        self.assertIn(self.raw,messages[-1]['content'])
        self.assertNotIn(self.sha,messages[-1]['content'])
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'42 days','sources':[item_handles(self.row)[0]]})}

        score = classify(self.row,self.truth,output,ITEM_VARIANT)

        self.assertTrue(score['completed_correct'])
        self.assertEqual(score['resolved_evidence'],[{'source_item_index':0,
            'raw_sha256':self.sha,'raw_range':[0,len(self.raw)],'quote':self.raw}])

    def test_whole_item_response_from_another_question_does_not_bind(self):
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'42 days','sources':[item_handles(self.row)[0]]})}
        self.row['question_id'] = 'another-retention-question'

        score = classify(self.row,self.truth,output,ITEM_VARIANT)

        self.assertTrue(score['value_correct'])
        self.assertFalse(score['source_bound'])
        self.assertFalse(score['completed_correct'])

    def test_whole_item_freeze_pins_identity_and_exact_catalog_before_inference(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            rows,truth = root/'rows.jsonl',root/'truth.json'
            rows.write_text(json.dumps(self.row)+'\n')
            truth.write_text(json.dumps({'cases':{'numeric':self.truth}}))

            freeze(root/'capture',rows,truth,'controlled','http://127.0.0.1:9999/v1',variants=[ITEM_VARIANT])

            plan = json.loads((root/'capture/registration.json').read_text())
            self.assertIn('source_reference_sha256',plan)
            self.assertIn('answer_matcher_sha256',plan)
            catalog = json.loads((root/'capture/item-reference-catalog.json').read_text())
            self.assertEqual(catalog['numeric'][item_handles(self.row)[0]]['raw_sha256'],self.sha)
            self.assertEqual(catalog['numeric'][item_handles(self.row)[0]]['text'],self.raw)

    def test_raw_item_freeze_stores_only_direct_source_projection(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            rows,truth = root/'rows.jsonl',root/'truth.json'
            rows.write_text(json.dumps(self.row)+'\n')
            truth.write_text(json.dumps({'cases':{'numeric':self.truth}}))

            freeze(root/'capture',rows,truth,'controlled','http://127.0.0.1:9999/v1',variants=[ITEM_VARIANT])

            projection = json.loads((root/'capture/projection.json').read_text())
            self.assertNotIn('graph',projection['numeric'][0])
            plan = json.loads((root/'capture/registration.json').read_text())
            self.assertEqual(plan['variant_treatments'][ITEM_VARIANT],
                ['whole_item_response_contract','source_identity','reviewed_support_coverage',
                 'whole_delivered_source','raw_source_only'])

    def test_mixed_freeze_retains_navigation_for_the_range_protocol(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            rows,truth = root/'rows.jsonl',root/'truth.json'
            rows.write_text(json.dumps(self.row)+'\n')
            truth.write_text(json.dumps({'cases':{'numeric':self.truth}}))

            freeze(root/'capture',rows,truth,'controlled','http://127.0.0.1:9999/v1',
                   variants=[VARIANT,ITEM_VARIANT])

            projection = json.loads((root/'capture/projection.json').read_text())
            self.assertIn('graph',projection['numeric'][0])
            prompts = json.loads((root/'capture/prompts.json').read_text())['numeric']
            self.assertEqual(len(prompts[VARIANT]),3)
            self.assertEqual(len(prompts[ITEM_VARIANT]),2)

    def test_retired_whole_item_registration_is_rejected_before_capture_creation(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            rows,truth = root/'rows.jsonl',root/'truth.json'
            rows.write_text(json.dumps(self.row)+'\n')
            truth.write_text(json.dumps({'cases':{'numeric':self.truth}}))

            with self.assertRaises(ValueError):
                freeze(root/'capture',rows,truth,'controlled','http://127.0.0.1:9999/v1',
                       variants=['source-item-final-first'])

            self.assertFalse((root/'capture').exists())

    def test_whole_item_freeze_rejects_gold_requiring_an_oversized_item(self):
        raw = 'A'*8001
        self.row['memory_context'] = [{'type':'text','value':raw}]
        self.truth['evidence_alternatives'] = [[{'source_item_index':0,
            'raw_sha256':hashlib.sha256(raw.encode()).hexdigest(),'raw_range':[0,1]}]]
        with TemporaryDirectory() as directory:
            root = Path(directory)
            rows,truth = root/'rows.jsonl',root/'truth.json'
            rows.write_text(json.dumps(self.row)+'\n')
            truth.write_text(json.dumps({'cases':{'numeric':self.truth}}))
            with self.assertRaisesRegex(ValueError,'legal whole-item'):
                freeze(root/'capture',rows,truth,'controlled','http://127.0.0.1:9999/v1',variants=[ITEM_VARIANT])
            self.assertFalse((root/'capture').exists())

    def test_whole_item_freeze_rejects_non_scalar_decoded_text(self):
        raw = r'"X\ud800Y"'
        self.row['memory_context'] = [{'type':'text','value':raw}]
        self.truth['evidence_alternatives'] = [[{'source_item_index':0,
            'raw_sha256':hashlib.sha256(raw.encode()).hexdigest(),'raw_range':[1,2]}]]
        with TemporaryDirectory() as directory:
            root = Path(directory)
            rows,truth = root/'rows.jsonl',root/'truth.json'
            rows.write_text(json.dumps(self.row)+'\n')
            truth.write_text(json.dumps({'cases':{'numeric':self.truth}}))
            with self.assertRaisesRegex(ValueError,'Unicode scalar'):
                freeze(root/'capture',rows,truth,'controlled','http://127.0.0.1:9999/v1',variants=[ITEM_VARIANT])
            self.assertFalse((root/'capture').exists())

    def test_whole_item_freeze_rejects_nine_required_source_items(self):
        self.row['memory_context'] = [{'type':'text','value':str(index)} for index in range(9)]
        self.truth['evidence_alternatives'] = [[{'source_item_index':index,
            'raw_sha256':hashlib.sha256(str(index).encode()).hexdigest(),'raw_range':[0,1]}
            for index in range(9)]]
        with TemporaryDirectory() as directory:
            root = Path(directory)
            rows,truth = root/'rows.jsonl',root/'truth.json'
            rows.write_text(json.dumps(self.row)+'\n')
            truth.write_text(json.dumps({'cases':{'numeric':self.truth}}))
            with self.assertRaisesRegex(ValueError,'legal whole-item'):
                freeze(root/'capture',rows,truth,'controlled','http://127.0.0.1:9999/v1',variants=[ITEM_VARIANT])
            self.assertFalse((root/'capture').exists())

    def test_mixed_protocol_freeze_validates_whole_item_oracle_too(self):
        raw = 'A'*8001
        self.row['memory_context'] = [{'type':'text','value':raw}]
        self.truth['evidence_alternatives'] = [[{'source_item_index':0,
            'raw_sha256':hashlib.sha256(raw.encode()).hexdigest(),'raw_range':[0,1]}]]
        with TemporaryDirectory() as directory:
            root = Path(directory)
            rows,truth = root/'rows.jsonl',root/'truth.json'
            rows.write_text(json.dumps(self.row)+'\n')
            truth.write_text(json.dumps({'cases':{'numeric':self.truth}}))
            with self.assertRaisesRegex(ValueError,'legal whole-item'):
                freeze(root/'capture',rows,truth,'controlled','http://127.0.0.1:9999/v1',variants=[VARIANT,ITEM_VARIANT])
            self.assertFalse((root/'capture').exists())

    def test_multiple_proof_spans_in_one_item_count_its_budget_once(self):
        raw = 'A'*8000
        self.row['memory_context'] = [{'type':'text','value':raw}]
        identity = {'source_item_index':0,'raw_sha256':hashlib.sha256(raw.encode()).hexdigest()}
        self.truth['evidence_alternatives'] = [[{**identity,'raw_range':[0,1]},
                                               {**identity,'raw_range':[7999,8000]}]]
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'42 days','sources':item_handles(self.row)})}

        score = classify(self.row,self.truth,output,ITEM_VARIANT)

        self.assertTrue(score['completed_correct'])
        self.assertEqual(len(score['resolved_evidence']),1)

    def test_whole_item_freeze_accepts_a_legal_alternative_to_oversized_proof(self):
        raw = 'A'*8001
        self.row['memory_context'].append({'type':'text','value':raw})
        self.truth['evidence_alternatives'].append([{'source_item_index':1,
            'raw_sha256':hashlib.sha256(raw.encode()).hexdigest(),'raw_range':[0,1]}])
        with TemporaryDirectory() as directory:
            root = Path(directory)
            rows,truth = root/'rows.jsonl',root/'truth.json'
            rows.write_text(json.dumps(self.row)+'\n')
            truth.write_text(json.dumps({'cases':{'numeric':self.truth}}))

            freeze(root/'capture',rows,truth,'controlled','http://127.0.0.1:9999/v1',variants=[ITEM_VARIANT])

            self.assertTrue((root/'capture/registration.json').is_file())

    def test_whole_item_capture_rejects_tampered_catalog_before_claim_or_inference(self):
        import asyncio
        from unittest.mock import patch
        from evals.longmemeval_v2.reader_protocol import capture
        with TemporaryDirectory() as directory:
            root = Path(directory)
            rows,truth = root/'rows.jsonl',root/'truth.json'
            rows.write_text(json.dumps(self.row)+'\n')
            truth.write_text(json.dumps({'cases':{'numeric':self.truth}}))
            freeze(root/'capture',rows,truth,'controlled','http://127.0.0.1:9999/v1',variants=[ITEM_VARIANT])
            (root/'capture/item-reference-catalog.json').write_text('{}')

            with patch('evals.longmemeval_v2.reader_protocol.local_async_client',
                       side_effect=AssertionError('tampered catalog reached inference')) as client:
                with self.assertRaisesRegex(ValueError,'reference catalog'):
                    asyncio.run(capture(root/'capture'))
                client.assert_not_called()
            self.assertFalse((root/'capture/capture-started.json').exists())

    def test_whole_item_unknown_handle_cannot_bind_an_answer(self):
        output = {'finish_reason':'stop','response_raw':'{"answer":"42 days","sources":["item-99"]}'}
        score = classify(self.row,self.truth,output,ITEM_VARIANT)
        self.assertFalse(score['source_bound'])
        self.assertFalse(score['completed_correct'])

    def test_whole_item_duplicate_handles_are_rejected(self):
        output = {'finish_reason':'stop','response_raw':json.dumps({'answer':'42 days','sources':[item_handles(self.row)[0],item_handles(self.row)[0]]})}
        score = classify(self.row,self.truth,output,ITEM_VARIANT)
        self.assertFalse(score['source_bound'])
        self.assertFalse(score['completed_correct'])

    def test_whole_item_same_text_from_another_source_does_not_cover_proof(self):
        self.row['memory_context'].append({'type':'text','value':self.raw})
        output = {'finish_reason':'stop','response_raw':json.dumps({'answer':'42 days','sources':[item_handles(self.row)[1]]})}
        score = classify(self.row,self.truth,output,ITEM_VARIANT)
        self.assertTrue(score['source_bound'])
        self.assertFalse(score['proof_covered'])
        self.assertFalse(score['completed_correct'])

    def test_whole_item_extra_unrelated_source_is_not_free_support(self):
        self.row['memory_context'].append({'type':'text','value':'An unrelated source.\n'})
        output = {'finish_reason':'stop','response_raw':json.dumps({'answer':'42 days','sources':[item_handles(self.row)[0],item_handles(self.row)[1]]})}
        score = classify(self.row,self.truth,output,ITEM_VARIANT)
        self.assertFalse(score['proof_covered'])
        self.assertFalse(score['completed_correct'])

    def test_whole_item_wrong_unit_is_not_rescued_by_full_proof(self):
        output = {'finish_reason':'stop','response_raw':json.dumps({'answer':'42 hours','sources':[item_handles(self.row)[0]]})}
        score = classify(self.row,self.truth,output,ITEM_VARIANT)
        self.assertTrue(score['proof_covered'])
        self.assertFalse(score['value_correct'])
        self.assertFalse(score['completed_correct'])

    def test_whole_item_aggregate_budget_cannot_be_bypassed_by_short_handles(self):
        self.row['memory_context'] = [{'type':'text','value':'A'*4001},{'type':'text','value':'B'*4001}]
        self.truth['evidence_alternatives'] = [[
            {'source_item_index':0,'raw_sha256':hashlib.sha256(('A'*4001).encode()).hexdigest(),'raw_range':[0,1]}]]
        output = {'finish_reason':'stop','response_raw':json.dumps({'answer':'42 days','sources':[item_handles(self.row)[0],item_handles(self.row)[1]]})}
        score = classify(self.row,self.truth,output,ITEM_VARIANT)
        self.assertFalse(score['source_bound'])
        self.assertFalse(score['completed_correct'])

    def test_whole_item_exact_budget_boundary_is_accepted(self):
        raw = 'A'*8000
        self.row['memory_context'] = [{'type':'text','value':raw}]
        self.truth['evidence_alternatives'] = [[{'source_item_index':0,
            'raw_sha256':hashlib.sha256(raw.encode()).hexdigest(),'raw_range':[0,8000]}]]
        output = {'finish_reason':'stop','response_raw':json.dumps({'answer':'42 days','sources':[item_handles(self.row)[0]]})}
        score = classify(self.row,self.truth,output,ITEM_VARIANT)
        self.assertTrue(score['completed_correct'])

    def test_whole_item_escaped_unicode_maps_to_exact_raw_item(self):
        raw = r'"A\n\ud83d\ude00é\nB"'
        self.row['memory_context'] = [{'type':'text','value':raw}]
        self.truth = {'answer_kind':'single_value','accepted_answers':['é'],
                      'evidence_alternatives':[[{'source_item_index':0,
                          'raw_sha256':hashlib.sha256(raw.encode()).hexdigest(),'raw_range':[4,19]}]]}
        output = {'finish_reason':'stop','response_raw':json.dumps({'answer':'é','sources':[item_handles(self.row)[0]]})}
        score = classify(self.row,self.truth,output,ITEM_VARIANT)
        self.assertTrue(score['completed_correct'])
        self.assertEqual(score['resolved_evidence'][0]['quote'],'A\n😀é\nB')
        self.assertEqual(score['resolved_evidence'][0]['raw_range'],[1,20])

    def test_whole_item_conflicting_scope_does_not_rescue_a_guessed_successor(self):
        raw = "[1] toolbar 'View'\n  [2] button 'Blue'\n  [3] button 'Red'\n[4] toolbar 'View'\n  [5] button 'Blue'\n  [6] button 'Gold'\n[7] heading 'End'\n"
        self.row['memory_context'] = [{'type':'text','value':raw}]
        self.row['question_text'] = 'What follows Blue in View?'
        self.truth = {'answer_kind':'single_value','accepted_answers':['UNKNOWN'],
                      'evidence_alternatives':[[{'source_item_index':0,
                          'raw_sha256':hashlib.sha256(raw.encode()).hexdigest(),'raw_range':[0,len(raw)]}]]}
        output = {'finish_reason':'stop','response_raw':json.dumps({'answer':'Red','sources':[item_handles(self.row)[0]]})}
        score = classify(self.row,self.truth,output,ITEM_VARIANT)
        self.assertTrue(score['proof_covered'])
        self.assertFalse(score['completed_correct'])

    def test_whole_item_unterminated_tail_is_not_repaired_into_complete_source(self):
        raw = "[1] toolbar 'View'\n  [2] button 'Blue'\n  [3] button 'Re"
        self.row['memory_context'] = [{'type':'text','value':raw}]
        self.truth = {'answer_kind':'single_value','accepted_answers':['UNKNOWN'],
                      'evidence_alternatives':[[{'source_item_index':0,
                          'raw_sha256':hashlib.sha256(raw.encode()).hexdigest(),'raw_range':[0,len(raw)]}]]}
        output = {'finish_reason':'stop','response_raw':json.dumps({'answer':'UNKNOWN','sources':[item_handles(self.row)[0]]})}
        score = classify(self.row,self.truth,output,ITEM_VARIANT)
        self.assertTrue(score['completed_correct'])
        self.assertEqual(score['resolved_evidence'][0]['quote'],raw)
        self.assertIn('final line terminated: false',protocol_messages(self.row,ITEM_VARIANT)[-1]['content'])

    def test_whole_item_truncated_generation_is_not_a_completion(self):
        output = {'finish_reason':'length','response_raw':json.dumps({'answer':'42 days','sources':[item_handles(self.row)[0]]})}
        score = classify(self.row,self.truth,output,ITEM_VARIANT)
        self.assertTrue(score['proof_covered'])
        self.assertFalse(score['completed_correct'])

    def test_response_limits_are_disclosed_to_the_reader(self):
        instruction = protocol_messages(self.row,VARIANT)[0]['content']
        self.assertIn('one to eight',instruction)
        self.assertIn('8000 decoded scalars',instruction)

    def test_source_bound_answer_returns_literal_source_and_original_range(self):
        messages = protocol_messages(self.row, VARIANT)
        handle = 's0-' + self.sha
        self.assertIn(handle, messages[-1]['content'])
        self.assertIn('1: Retention: 42 days.', messages[-1]['content'])
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'42 days','evidence':[{'source':handle,'lines':[1,1]}]})}
        score = classify(self.row, self.truth, output, VARIANT)
        self.assertTrue(score['completed_correct'])
        self.assertTrue(score['source_bound'])
        self.assertTrue(score['proof_covered'])
        self.assertEqual(score['resolved_evidence'][0]['quote'], 'Retention: 42 days.\n')
        self.assertEqual(score['resolved_evidence'][0]['raw_range'], [0,20])

    def test_boolean_line_is_not_a_source_location(self):
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'42 days','evidence':[{'source':'s0-'+self.sha,'lines':[True,1]}]})}
        score = classify(self.row, self.truth, output, VARIANT)
        self.assertFalse(score['source_bound'])
        self.assertFalse(score['completed_correct'])

    def test_duplicate_json_answer_cannot_silently_replace_wrong_value(self):
        output = {'finish_reason':'stop','response_raw':(
            '{"answer":"99 days","answer":"42 days","evidence":'
            + json.dumps([{'source':'s0-'+self.sha,'lines':[1,1]}]) + '}')}
        score = classify(self.row, self.truth, output, VARIANT)
        self.assertFalse(score['format_valid'])
        self.assertFalse(score['completed_correct'])

    def test_extra_evidence_fields_cannot_be_ignored(self):
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'42 days','evidence':[{'source':'s0-'+self.sha,'lines':[1,1],
                                           'quote':'fabricated'}]})}
        score = classify(self.row, self.truth, output, VARIANT)
        self.assertFalse(score['format_valid'])
        self.assertFalse(score['completed_correct'])

    def test_duplicate_source_references_do_not_count_as_independent_evidence(self):
        ref = {'source':'s0-'+self.sha,'lines':[1,1]}
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'42 days','evidence':[ref,ref]})}
        score = classify(self.row, self.truth, output, VARIANT)
        self.assertFalse(score['source_bound'])
        self.assertFalse(score['completed_correct'])

    def test_correct_answer_with_unrelated_source_line_fails_support(self):
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'42 days','evidence':[{'source':'s0-'+self.sha,'lines':[2,2]}]})}
        score = classify(self.row, self.truth, output, VARIANT)
        self.assertTrue(score['source_bound'])
        self.assertTrue(score['value_correct'])
        self.assertFalse(score['proof_covered'])
        self.assertFalse(score['completed_correct'])

    def test_correct_quote_from_foreign_source_does_not_cover_reviewed_proof(self):
        self.row['memory_context'].append({'type':'text','value':self.raw})
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'42 days','evidence':[{'source':'s1-'+self.sha,'lines':[1,1]}]})}
        score = classify(self.row, self.truth, output, VARIANT)
        self.assertTrue(score['source_bound'])
        self.assertFalse(score['proof_covered'])
        self.assertFalse(score['completed_correct'])

    def test_unknown_source_handle_cannot_bind_evidence(self):
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'42 days','evidence':[{'source':'s0-'+('0'*64),'lines':[1,1]}]})}
        score = classify(self.row, self.truth, output, VARIANT)
        self.assertFalse(score['source_bound'])
        self.assertFalse(score['completed_correct'])

    def test_wrong_number_with_valid_source_is_not_rescued(self):
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'99 days','evidence':[{'source':'s0-'+self.sha,'lines':[1,1]}]})}
        score = classify(self.row, self.truth, output, VARIANT)
        self.assertTrue(score['proof_covered'])
        self.assertFalse(score['value_correct'])
        self.assertFalse(score['completed_correct'])

    def test_adjacent_line_references_jointly_cover_one_reviewed_range(self):
        self.truth['evidence_alternatives'][0][0]['raw_range'] = [0,39]
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'42 days','evidence':[{'source':'s0-'+self.sha,'lines':[1,1]},
                                          {'source':'s0-'+self.sha,'lines':[2,2]}]})}
        score = classify(self.row, self.truth, output, VARIANT)
        self.assertTrue(score['proof_covered'])
        self.assertTrue(score['completed_correct'])

    def test_reference_budget_is_enforced_on_actual_delivered_text(self):
        raw = 'A' * 8001
        sha = hashlib.sha256(raw.encode()).hexdigest()
        self.row['memory_context'] = [{'type':'text','value':raw}]
        self.truth['evidence_alternatives'] = [[{'source_item_index':0,'raw_sha256':sha,'raw_range':[0,1]}]]
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'42 days','evidence':[{'source':'s0-'+sha,'lines':[1,1]}]})}
        score = classify(self.row, self.truth, output, VARIANT)
        self.assertFalse(score['source_bound'])
        self.assertFalse(score['completed_correct'])

    def test_escaped_unicode_lines_resolve_to_original_scalar_ranges(self):
        raw = r'A\n\ud83d\ude00é\nB'
        sha = hashlib.sha256(raw.encode()).hexdigest()
        self.row['memory_context'] = [{'type':'text','value':raw}]
        self.truth = {'answer_kind':'single_value','accepted_answers':['é'],
                      'evidence_alternatives':[[{'source_item_index':0,'raw_sha256':sha,'raw_range':[3,18]}]]}
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'é','evidence':[{'source':'s0-'+sha,'lines':[2,2]}]})}
        score = classify(self.row, self.truth, output, VARIANT)
        self.assertTrue(score['completed_correct'])
        self.assertEqual(score['resolved_evidence'][0]['quote'], '😀é\n')
        self.assertEqual(score['resolved_evidence'][0]['raw_range'], [3,18])

    def test_error_or_truncation_cannot_turn_verified_evidence_into_completion(self):
        output = {'finish_reason':'length','response_raw':json.dumps({
            'answer':'42 days','evidence':[{'source':'s0-'+self.sha,'lines':[1,1]}]})}
        score = classify(self.row, self.truth, output, VARIANT)
        self.assertTrue(score['proof_covered'])
        self.assertFalse(score['completed_correct'])

    def test_empty_gold_proof_is_invalid_evidence_instead_of_vacuous_success(self):
        self.truth['evidence_alternatives'] = [[]]
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'42 days','evidence':[{'source':'s0-'+self.sha,'lines':[1,1]}]})}
        with self.assertRaisesRegex(ValueError, 'support oracle'):
            classify(self.row, self.truth, output, VARIANT)

    def test_scalar_answer_gold_is_rejected_before_any_answer_can_pass(self):
        bad = {**self.truth,'accepted_answers':'42 days'}
        with self.assertRaisesRegex(ValueError,'answer oracle'):
            classify(self.row,bad,None,VARIANT)

    def test_freeze_rejects_duplicate_answer_oracle_fields(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            rows, truth = root/'rows.jsonl', root/'truth.json'
            rows.write_text(json.dumps(self.row)+'\n')
            body = json.dumps({'cases':{self.row['question_id']:self.truth}})
            body = body.replace('"accepted_answers": ["42 days"]',
                                '"accepted_answers": ["99 days"], "accepted_answers": ["42 days"]')
            truth.write_text(body)
            with self.assertRaisesRegex(ValueError,'duplicate JSON key'):
                freeze(root/'capture',rows,truth,'controlled','http://127.0.0.1:9999/v1',variants=[VARIANT])
            self.assertFalse((root/'capture').exists())

    def test_source_capture_requires_scorer_pins_before_claiming_a_model_attempt(self):
        import asyncio
        from unittest.mock import patch
        from evals.longmemeval_v2.reader_protocol import capture
        with TemporaryDirectory() as directory:
            root = Path(directory)
            rows, truth = root/'rows.jsonl', root/'truth.json'
            rows.write_text(json.dumps(self.row)+'\n')
            truth.write_text(json.dumps({'cases':{self.row['question_id']:self.truth}}))
            freeze(root/'capture',rows,truth,'controlled','http://127.0.0.1:9999/v1',variants=[VARIANT])
            registration = root/'capture/registration.json'
            plan = json.loads(registration.read_text())
            del plan['source_reference_sha256']
            registration.write_text(json.dumps(plan))
            with patch('evals.longmemeval_v2.reader_protocol.local_async_client',
                       side_effect=AssertionError('invalid registration reached inference')) as client:
                with self.assertRaisesRegex(ValueError,'scorer identity'):
                    asyncio.run(capture(root/'capture'))
                client.assert_not_called()
            self.assertFalse((root/'capture/capture-started.json').exists())

    def test_freeze_rejects_a_missing_source_support_oracle_before_output_directory(self):
        from tempfile import TemporaryDirectory
        from pathlib import Path
        with TemporaryDirectory() as directory:
            root = Path(directory)
            rows, truth = root/'rows.jsonl', root/'truth.json'
            rows.write_text(json.dumps(self.row)+'\n')
            truth.write_text(json.dumps({'cases':{self.row['question_id']:{
                'accepted_answers':['42 days'],'answer_kind':'single_value'}}}))
            with self.assertRaisesRegex(ValueError,'support oracle'):
                freeze(root/'capture',rows,truth,'controlled','http://127.0.0.1:9999/v1',variants=[VARIANT])
            self.assertFalse((root/'capture').exists())

    def test_freeze_pins_source_contract_implementation_and_reference_catalog(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            rows, truth = root/'rows.jsonl', root/'truth.json'
            rows.write_text(json.dumps(self.row)+'\n')
            truth.write_text(json.dumps({'cases':{'numeric':self.truth}}))
            freeze(root/'capture',rows,truth,'controlled','http://127.0.0.1:9999/v1',variants=[VARIANT])
            plan = json.loads((root/'capture/registration.json').read_text())
            self.assertIn('source_reference_sha256',plan)
            self.assertEqual(plan['variant_treatments'][VARIANT],
                             ['source_range_response_contract','source_identity','reviewed_support_coverage','numbered_source_text','source_graph'])
            catalog = json.loads((root/'capture/reference-catalog.json').read_text())
            self.assertEqual(catalog['numeric']['s0-'+self.sha]['lines'][0]['raw_range'],[0,20])

    def test_extra_unrelated_citation_does_not_become_support_for_the_answer(self):
        output = {'finish_reason':'stop','response_raw':json.dumps({
            'answer':'42 days','evidence':[{'source':'s0-'+self.sha,'lines':[1,1]},
                                          {'source':'s0-'+self.sha,'lines':[2,2]}]})}
        score = classify(self.row, self.truth, output, VARIANT)
        self.assertFalse(score['proof_covered'])
        self.assertFalse(score['completed_correct'])

    def test_prompt_does_not_hide_unterminated_source_line(self):
        self.row['memory_context'] = [{'type':'text','value':"[1] toolbar 'View'\n  [2] button 'Clipp"}]
        text = protocol_messages(self.row,VARIANT)[-1]['content']
        self.assertIn('final line terminated: false',text)
        self.assertIn("2:   [2] button 'Clipp",text)

    def test_source_navigation_is_retained_and_question_independent(self):
        self.row['memory_context'] = [{'type':'text','value':
            "[1] toolbar 'View'\n  [2] button 'Blue'\n  [3] button 'Red'\n[4] heading 'End'\n"}]
        first = protocol_messages(self.row,VARIANT)
        self.row['question_text'] = 'A completely different question?'
        second = protocol_messages(self.row,VARIANT)
        self.assertIn('DERIVED NAVIGATION',first[1]['content'])
        self.assertEqual(first[1],second[1])
        self.assertIn('observed',first[1]['content'])
        self.assertIn('Red',first[1]['content'])


if __name__ == '__main__':
    unittest.main()
