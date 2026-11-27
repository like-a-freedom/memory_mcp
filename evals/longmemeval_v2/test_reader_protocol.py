"""Independent protocol controls; no model outputs are repaired."""
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from evals.longmemeval_v2.reader_protocol import classify, freeze, project_sources, protocol_messages, summarize


class ReaderProtocolTests(unittest.TestCase):
    def test_backspace_truncation_refusal_and_fabricated_quote_are_separate(self):
        row = {'question_id':'a', 'question_text':'Which option?',
               'prompt_messages':[{'role':'system','content':'Explain in \boxed{}'}],
               'memory_context':[{'type':'text','value':'No option exists.'}]}
        truth = {'answer_kind':'false_premise','accepted_answers':['None']}
        result = classify(row, truth, {'response_raw':r'\boxed{None}', 'finish_reason':'length'})
        self.assertTrue(result['upstream_prompt_control_character'])
        self.assertTrue(result['format_valid'])
        self.assertTrue(result['value_correct'])
        self.assertTrue(result['refusal_correct'])
        self.assertTrue(result['truncated'])
        self.assertFalse(result['completed_correct'])
        forged = classify(row, truth, {'response_raw':r'\boxed{None}'+'\nQUOTE: approved', 'finish_reason':'stop'})
        self.assertFalse(forged['quote_exact'])
        correct = classify(row, truth, {'response_raw':r'\boxed{None}'+'\nQUOTE: No option exists.', 'finish_reason':'stop'})
        self.assertTrue(correct['quote_exact'])

    def test_all_seven_cases_remain_in_denominator_when_output_is_missing(self):
        rows = [{'question_id':str(i),'question_text':'q','prompt_messages':[], 'memory_context':[]} for i in range(7)]
        truth = {str(i):{'answer_kind':'single_value','accepted_answers':['yes']} for i in range(7)}
        result = summarize(rows, truth, {'0':{'response_raw':r'\boxed{yes}','finish_reason':'stop'}})
        self.assertEqual(result['selected'], 7)
        self.assertEqual(result['completed_correct'], 1)
        self.assertEqual(result['unobserved'], 6)

    def test_corrected_instruction_is_literal_and_never_includes_gold_answers(self):
        row = {'question_text':'What is selected?', 'prompt_messages':[],
               'memory_context':[{'type':'text','value':'Available: violet'}]}
        messages = protocol_messages(row, 'final-first')
        self.assertNotIn('\x08', str(messages))
        self.assertIn(r'\boxed{', messages[0]['content'])
        self.assertIn('What is selected?', messages[-1]['content'])
        self.assertIn('Available: violet', messages[-1]['content'])

    def test_projection_is_reversible_and_retains_each_original_item_identity(self):
        original = 'one\\ntwo\\tthree'
        items = [{'type':'text','value':original}, {'type':'text','value':'second'}]
        projection = project_sources(items)
        first = projection[0]
        self.assertEqual(first['source_item_index'], 0)
        self.assertEqual(first['text'], 'one\ntwo\tthree')
        # Every emitted character points to the original contiguous raw token.
        self.assertEqual(original[first['raw_offsets'][3][0]:first['raw_offsets'][3][1]], r'\n')
        self.assertEqual(projection[1]['source_item_index'], 1)

    def test_escaped_supplementary_character_can_be_dispatched_with_exact_raw_offsets(self):
        raw = r'X\ud83d\ude00\u00e9Y'
        items = [{'type':'text','value':raw}]
        row = {'question_text':'What is shown?', 'memory_context':items}
        messages = protocol_messages(row,'structured-final-first')
        try:
            body = json.dumps(messages,ensure_ascii=False).encode('utf-8')
        except UnicodeEncodeError as error:
            self.fail(f'projected prompt is not UTF-8 encodable ({error.reason})')
        expected = 'X\U0001f600\u00e9Y'
        self.assertIn(expected.encode('utf-8'), body)
        projection, = project_sources(items, structured=True)
        self.assertEqual(projection['text'], expected)
        self.assertEqual(projection['raw_offsets'], [[0, 1], [1, 13], [13, 19], [19, 20]])

    def test_raw_protocol_cannot_quote_an_undelivered_decoded_projection(self):
        row = {'question_id':'projection-control','question_text':'Which?', 'prompt_messages':[],
               'memory_context':[{'type':'text','value':r'He said \"Violet\".'}]}
        truth = {'answer_kind':'single_value','accepted_answers':['Second']}
        output = {'response_raw':r'\boxed{Second}'+'\nQUOTE: He said "Violet".','finish_reason':'stop'}
        self.assertFalse(classify(row,truth,output)['quote_exact'])

    def test_readable_protocol_can_quote_its_delivered_decoded_projection(self):
        row = {'question_id':'projection-control','question_text':'Which?', 'prompt_messages':[],
               'memory_context':[{'type':'text','value':r'He said \"Violet\".'}]}
        truth = {'answer_kind':'single_value','accepted_answers':['Violet']}
        output = {'response_raw':r'\boxed{Violet}'+'\nQUOTE: He said "Violet".','finish_reason':'stop'}
        self.assertTrue(classify(row,truth,output,'readable-final-first')['quote_exact'])

    def test_structured_reader_preserves_source_and_annotates_navigation_peer_order(self):
        raw = (r"[1] main ''\n\t[2] list ''\n\t\t[3] listitem ''\n"
               r"\t\t\t[4] link 'Iris'\n\t\t\t\tStaticText 'Iris'\n"
               r"\t\t[5] listitem ''\n\t\t\t[6] link 'Juniper'\n\t\t\t\tStaticText 'Juniper'\n"
               r"\t\t[7] listitem ''\n\t\t\t[8] link 'Rose'\n"
               r"\t[9] heading 'Directory'\n")
        row = {'question_text':'Which tabs are shown?', 'prompt_messages':[],
               'memory_context':[{'type':'text','value':raw}, {'type':'text','value':'Other text'}]}
        try:
            messages = protocol_messages(row,'structured-final-first')
        except ValueError as error:
            self.fail(str(error))
        evidence = messages[-1]['content']
        self.assertIn(project_sources(row['memory_context'])[0]['text'], evidence)
        self.assertIn('SOURCE ITEM 1:\nOther text', evidence)
        self.assertIn("[2] list '' (scope ends at an observed dedent, parsed peer count 3):\n"
                      "1. [4] link 'Iris'\n2. [6] link 'Juniper'\n3. [8] link 'Rose'", evidence)
        self.assertIn("[4] link 'Iris' has nested StaticText", evidence)

    def test_structured_source_marks_unobserved_boundary_and_retains_raw_location(self):
        raw = r"[1] toolbar 'Main'\n\t[2] button 'Crème'\n\t[3] button 'Clipp"
        source = project_sources([{'type':'text','value':raw}], structured=True)[0]
        toolbar, = source['structure']
        observed, = toolbar['ordered_peers']
        start, end = observed['raw_range']
        self.assertEqual(raw[start:end], "[2] button 'Crème'")
        self.assertFalse(toolbar['dedent_observed'])
        self.assertIn("[3] button 'Clipp", source['text'])
        row = {'question_text':'What is in this view?', 'memory_context':[{'type':'text','value':raw}]}
        prompt = protocol_messages(row,'structured-final-first')[-1]['content']
        self.assertIn("scope boundary unproven, parsed peer count 1):\n1. [2] button 'Crème'", prompt)
        self.assertNotIn("2. [3] button 'Clipp", prompt)

    def test_structured_source_does_not_join_actions_across_scope_gap(self):
        raw = r"[1] toolbar 'Main'\n\t[2] button 'Iris'\n…\n\t[3] button 'Rose'\n"
        source = project_sources([{'type':'text','value':raw}], structured=True)[0]
        toolbar, = source['structure']
        self.assertEqual([peer['text'] for peer in toolbar['ordered_peers']], ["[2] button 'Iris'"])
        self.assertFalse(toolbar['dedent_observed'])

    def test_structured_reader_keeps_source_quote_and_refusal_contract(self):
        raw = r"[1] toolbar 'Main'\n\t[2] button 'Iris'\n\t[3] button 'Rose'\n[4] heading 'Report'\n"
        row = {'prompt_messages':[], 'memory_context':[{'type':'text','value':raw}]}
        truth = {'answer_kind':'false_premise','accepted_answers':['None']}
        output = {'response_raw':r'\boxed{None}'+'\nQUOTE: [2] button \'Iris\'', 'finish_reason':'stop'}
        result = classify(row,truth,output,'structured-final-first')
        self.assertTrue(result['refusal_correct'])
        self.assertTrue(result['quote_exact'])
        annotation = {'response_raw':r'\boxed{None}'+'\nQUOTE: scope ends at an observed dedent', 'finish_reason':'stop'}
        self.assertFalse(classify(row,truth,annotation,'structured-final-first')['quote_exact'])

    def test_structured_reader_distinguishes_missing_evidence_from_refutation(self):
        row = {'prompt_messages':[], 'memory_context':[{'type':'text','value':'No toolbar snapshot was recorded.'}]}
        truth = {'answer_kind':'unknown','accepted_answers':['UNKNOWN']}
        missing = {'response_raw':r'\boxed{UNKNOWN}', 'finish_reason':'stop'}
        refuted = {'response_raw':r'\boxed{None}', 'finish_reason':'stop'}
        self.assertTrue(classify(row,truth,missing,'structured-final-first')['value_correct'])
        self.assertFalse(classify(row,truth,refuted,'structured-final-first')['value_correct'])

    def test_freeze_selects_declared_arms_and_no_retry(self):
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            rows = root/'rows.jsonl'
            truth = root/'truth.json'
            row = {'question_id':'synthetic', 'question_text':'Which tab?', 'prompt_messages':[], 'memory_context':[]}
            rows.write_text(json.dumps(row)+'\n')
            truth.write_text(json.dumps({'cases':{'synthetic':{'accepted_answers':['UNKNOWN'], 'answer_kind':'unknown'}}}))
            variants = ['readable-final-first', 'structured-final-first']
            try:
                freeze(root/'paired',rows,truth,'controlled','http://127.0.0.1:9999/v1',variants=variants)
            except TypeError as error:
                self.fail(str(error))
            plan = json.loads((root/'paired'/'registration.json').read_text())
            prompts = json.loads((root/'paired'/'prompts.json').read_text())
            self.assertEqual(plan['variants'], variants)
            self.assertEqual(list(prompts['synthetic']), variants)
            self.assertEqual(plan['max_retries'], 0)
            self.assertEqual(plan['variant_treatments']['structured-final-first'], ['evidence_structure', 'reasoning_instruction'])
            self.assertEqual(plan['sampling']['max_completion_tokens'], 256)


class OrderedRelationProtocolTests(unittest.TestCase):
    variant = 'ordered-relations-final-first'

    def prompt(self, raw, question='Which observation holds?'):
        row = {'question_text':question, 'memory_context':[{'type':'text','value':raw}]}
        return protocol_messages(row, self.variant)[1]['content']

    def test_complete_list_records_empty_between_peers_and_terminal_successor(self):
        raw = ("[1] list 'Minerals'\n  [2] listitem ''\n    [3] link 'Onyx'\n"
               "      StaticText 'Onyx'\n  [4] listitem ''\n    [5] link 'Jade'\n"
               "[6] heading 'Next section'\n")
        prompt = self.prompt(raw)
        self.assertIn(raw, prompt)
        self.assertIn("[3] link 'Onyx' -> [5] link 'Jade' | between: EMPTY", prompt)
        self.assertIn("After [5] link 'Jade' | successor: NONE", prompt)

    def test_clipped_tail_retains_observed_adjacency_without_asserting_terminal(self):
        raw = "[1] toolbar 'Edit'\n  [2] button 'Cut'\n  [3] button 'Paste'\n  [4] button 'Cli"
        prompt = self.prompt(raw)
        self.assertIn("[2] button 'Cut' -> [3] button 'Paste' | between: EMPTY", prompt)
        self.assertNotIn('successor: NONE', prompt)

    def test_intervening_nested_actions_cannot_disappear_from_a_proven_relation(self):
        raw = ("[1] toolbar 'Edit'\n  [2] button 'Cut'\n  [3] group 'More'\n"
               "    [4] button 'Preview'\n  [5] button 'Paste'\n[6] heading 'Other'\n")
        prompt = self.prompt(raw)
        self.assertNotIn('between: EMPTY', prompt)
        self.assertNotIn('successor: NONE', prompt)

    def test_same_parent_with_nested_group_in_list_row_does_not_prove_completeness(self):
        raw = ("[1] list 'Menu'\n  [2] listitem ''\n    [3] link 'A'\n"
               "    [4] group ''\n      [5] link 'B'\n  [6] listitem ''\n"
               "    [7] link 'C'\n[8] heading 'Other'\n")
        prompt = self.prompt(raw)
        self.assertNotIn('between: EMPTY', prompt)
        self.assertNotIn('successor: NONE', prompt)

    def test_different_scopes_do_not_join_and_annotations_ignore_question(self):
        raw = ("[1] toolbar 'File'\n  [2] button 'New'\n  [3] button 'Open'\n"
               "[4] toolbar 'Edit'\n  [5] button 'Copy'\n  [6] button 'Paste'\n[7] heading 'End'\n")
        evidence = self.prompt(raw, 'Where is Open?').split('MEMORY:\n', 1)[1]
        self.assertEqual(evidence, self.prompt(raw, 'Count the files').split('MEMORY:\n', 1)[1])
        self.assertIn("[2] button 'New' -> [3] button 'Open' | between: EMPTY", evidence)
        self.assertIn("[5] button 'Copy' -> [6] button 'Paste' | between: EMPTY", evidence)
        self.assertNotIn("[3] button 'Open' -> [5] button 'Copy'", evidence)

    def test_relations_in_frozen_projection_resolve_to_raw_unicode_ranges(self):
        raw = r"[1] toolbar 'Tools'\n\t[2] button '\ud83d\udc8e'\n\t[3] button 'Garnet'\n[4] heading 'End'\n"
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            row = {'question_id':'unicode','question_text':'Which?', 'prompt_messages':[],
                   'memory_context':[{'type':'text','value':raw}]}
            (root/'rows.jsonl').write_text(json.dumps(row)+'\n')
            (root/'truth.json').write_text(json.dumps({'cases':{'unicode':{
                'accepted_answers':['UNKNOWN'],'answer_kind':'unsupported'}}}))
            freeze(root/'run',root/'rows.jsonl',root/'truth.json','local','http://127.0.0.1:9999/v1',
                   variants=[self.variant])
            source, = json.loads((root/'run/projection.json').read_text())['unicode']
            group, = source['structure']
            left, right = group['immediate_pairs'][0]
            a, b = left['raw_range']
            self.assertEqual(raw[a:b], r"[2] button '\ud83d\udc8e'")
            self.assertEqual(left['text'], "[2] button '💎'")
            self.assertEqual(group['terminal_peer'], right)

    def test_virtualized_entries_from_larger_set_do_not_prove_empty_or_terminal(self):
        raw = ("[1] menu 'Files'\n  [2] menuitem 'First', posinset=1, setsize=5\n"
               "  [5] menuitem 'Later', posinset=4, setsize=5\n[8] heading 'Other'\n")
        prompt = self.prompt(raw)
        self.assertNotIn('between: EMPTY', prompt)
        self.assertNotIn('successor: NONE', prompt)

    def test_inconsistent_sibling_indentation_does_not_establish_adjacency(self):
        raw = "[1] toolbar 'Cut'\n    [2] button 'A'\n  [3] button 'B'\n[4] heading 'End'\n"
        prompt = self.prompt(raw)
        self.assertNotIn('between: EMPTY', prompt)
        self.assertNotIn('successor: NONE', prompt)


class SourceLedgerProtocolTests(unittest.TestCase):
    variant = 'source-ledger-final-first'

    def test_source_quotes_have_a_separate_channel_from_derived_relations(self):
        raw = "[1] toolbar 'Paper'\n  [2] button 'Fold'\n  [3] button 'Seal'\n[4] heading 'End'\n"
        row = {'question_text':'What follows Fold?', 'prompt_messages':[], 'memory_context':[{'type':'text','value':raw}]}
        messages = protocol_messages(row, self.variant)
        self.assertEqual(len(messages), 3)
        self.assertIn('NOT QUOTABLE', messages[1]['content'])
        self.assertIn('between: EMPTY', messages[1]['content'])
        self.assertIn(raw, messages[2]['content'])
        self.assertNotIn('between: EMPTY', messages[2]['content'])
        truth = {'accepted_answers':['Seal'], 'answer_kind':'answerable'}
        fabricated = classify(row,truth,{'response_raw':r'\boxed{Seal}'+'\nQUOTE: Fold -> Seal | between: EMPTY',
                                       'finish_reason':'stop'},self.variant)
        self.assertFalse(fabricated['quote_exact'])

    def test_conflicting_duplicate_anchors_preserve_both_occurrences(self):
        raw = ("[1] toolbar 'Routes'\n  [2] button 'Latch'\n  [3] button 'North'\n"
               "  [4] button 'Latch'\n  [5] button 'South'\n[6] heading 'End'\n")
        source, = project_sources([{'type':'text','value':raw}], anchor_ledger=True)
        group, = source['structure']
        label, = group['peer_name_occurrences']
        self.assertEqual(label['name'], 'Latch')
        self.assertEqual([o['line'] for o in label['occurrences']], [2, 4])
        self.assertEqual([o['role'] for o in label['occurrences']], ['button', 'button'])
        row = {'question_text':'What follows Latch?', 'memory_context':[{'type':'text','value':raw}]}
        notes = protocol_messages(row,self.variant)[1]['content']
        self.assertIn("LABEL 'Latch' has 2 occurrences in this scope", notes)
        self.assertIn("[2] button 'Latch' -> [3] button 'North'", notes)
        self.assertIn("[4] button 'Latch' -> [5] button 'South'", notes)

    def test_consistent_repeated_anchor_keeps_every_observed_same_successor(self):
        raw = ("[1] toolbar 'Routes'\n  [2] button 'Latch'\n  [3] button 'River'\n"
               "  [4] button 'Latch'\n  [5] button 'River'\n[6] heading 'End'\n")
        row = {'question_text':'What follows Latch?', 'memory_context':[{'type':'text','value':raw}]}
        notes = protocol_messages(row,self.variant)[1]['content']
        self.assertIn("[2] button 'Latch' -> [3] button 'River'",notes)
        self.assertIn("[4] button 'Latch' -> [5] button 'River'",notes)
        self.assertNotIn('answer UNKNOWN',notes)

    def test_same_labels_in_header_and_footer_are_separate_scopes(self):
        raw = ("[1] toolbar 'Header'\n  [2] button 'Latch'\n  [3] button 'North'\n"
               "[4] toolbar 'Footer'\n  [5] button 'Latch'\n  [6] button 'South'\n[7] heading 'End'\n")
        source, = project_sources([{'type':'text','value':raw}],anchor_ledger=True)
        self.assertEqual([g['parent']['text'] for g in source['structure']],
                         ["[1] toolbar 'Header'", "[4] toolbar 'Footer'"])
        self.assertEqual([g['peer_name_occurrences'] for g in source['structure']],[[],[]])

    def test_repeated_names_preserve_distinct_roles(self):
        raw = ("[1] toolbar 'Routes'\n  [2] button 'Latch'\n  [3] button 'River'\n"
               "  [4] link 'Latch'\n  [5] link 'Hill'\n[6] heading 'End'\n")
        source, = project_sources([{'type':'text','value':raw}],anchor_ledger=True)
        group, = source['structure']
        label, = group['peer_name_occurrences']
        self.assertEqual([o['role'] for o in label['occurrences']], ['button','link'])

    def test_opaque_child_is_not_removed_from_a_relation(self):
        raw = ("[1] toolbar 'Routes'\n  [2] button 'Latch'\n  [3] group 'Omitted controls'\n"
               "  [4] button 'River'\n[5] heading 'End'\n")
        row = {'question_text':'What follows Latch?', 'memory_context':[{'type':'text','value':raw}]}
        notes = protocol_messages(row,self.variant)[1]['content']
        self.assertNotIn('between: EMPTY',notes)
        self.assertNotIn('successor: NONE',notes)

    def test_clipped_duplicate_keeps_unresolved_occurrence_without_terminal_certainty(self):
        raw = ("[1] toolbar 'Routes'\n  [2] button 'Latch'\n  [3] button 'River'\n"
               "  [4] button 'Latch'\n  [5] button 'Unfin")
        source, = project_sources([{'type':'text','value':raw}],anchor_ledger=True)
        group, = source['structure']
        label, = group['peer_name_occurrences']
        self.assertEqual([o['line'] for o in label['occurrences']], [2,4])
        self.assertIsNone(group['terminal_peer'])
        self.assertFalse(group['dedent_observed'])

    def test_frozen_ledger_has_source_identity_offsets_and_treatment_registration(self):
        raw = r"[1] toolbar 'Routes'\n\t[2] button 'Latch'\n\t[3] button 'River'\n\t[4] button 'Latch'\n\t[5] button 'Hill'\n[6] heading 'End'\n"
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root/'rows.jsonl').write_text(json.dumps({'question_id':'synthetic','question_text':'Which?',
                'prompt_messages':[],'memory_context':[{'type':'text','value':raw}]})+'\n')
            (root/'truth.json').write_text(json.dumps({'cases':{'synthetic':{
                'accepted_answers':['UNKNOWN'],'answer_kind':'unknown'}}}))
            freeze(root/'run',root/'rows.jsonl',root/'truth.json','local','http://127.0.0.1:9999/v1',variants=[self.variant])
            projection, = json.loads((root/'run/projection.json').read_text())['synthetic']
            group, = projection['structure']
            label, = group['peer_name_occurrences']
            first, second = label['occurrences']
            self.assertEqual(raw[first['raw_range'][0]:first['raw_range'][1]], "[2] button 'Latch'")
            self.assertEqual(raw[second['raw_range'][0]:second['raw_range'][1]], "[4] button 'Latch'")
            self.assertEqual(projection['source_item_index'],0)
            plan = json.loads((root/'run/registration.json').read_text())
            self.assertEqual(plan['variant_treatments'][self.variant],
                             ['evidence_structure','ordered_peer_relations','anchor_occurrences','quote_channel_separation','reasoning_instruction'])


class SourceGraphProtocolTests(unittest.TestCase):
    variant = 'source-graph-final-first'

    def test_graph_preserves_literal_locations_and_observed_successor(self):
        raw = "[1] toolbar 'Main'\n  [2] button 'Birch'\n  [3] button 'Maple'\n[4] heading 'End'\n"
        row = {'question_text':'Which button follows Birch?', 'memory_context':[{'type':'text','value':raw}]}
        messages = protocol_messages(row, self.variant)
        graph = json.loads(messages[1]['content'].split('\n',1)[1])[0]
        group, = graph['groups']
        first, last = group['peers']
        self.assertEqual(first['name'], 'Birch')
        self.assertEqual(first['successor'], {'state':'observed','source_line':3,'name':'Maple'})
        self.assertEqual(last['successor'], {'state':'end'})
        self.assertEqual(raw[first['raw_range'][0]:first['raw_range'][1]], "[2] button 'Birch'")
        self.assertIn(raw, messages[-1]['content'])
        self.assertNotIn('between: EMPTY', messages[1]['content'])

    def test_same_value_successors_preserve_both_occurrences(self):
        raw = ("[1] toolbar 'Harbor'\n  [2] button 'Release'\n  [3] button 'River'\n"
               "  [4] button 'Release'\n  [5] button 'River'\n[6] heading 'End'\n")
        source, = project_sources([{'type':'text','value':raw}],source_graph=True)
        group, = source['graph']
        repeated, river = group['repeated_labels']
        self.assertEqual(river['occurrence_lines'], [3,5])
        self.assertEqual(repeated, {'role':'button','name':'Release','occurrence_lines':[2,4],
                                   'successor_values':['River'],'unresolved_occurrence_lines':[]})

    def test_conflicting_successors_are_not_collapsed_by_repeated_name(self):
        raw = ("[1] toolbar 'Harbor'\n  [2] button 'Release'\n  [3] button 'River'\n"
               "  [4] button 'Release'\n  [5] button 'Hill'\n[6] heading 'End'\n")
        source, = project_sources([{'type':'text','value':raw}],source_graph=True)
        group, = source['graph']
        repeated, = group['repeated_labels']
        self.assertEqual(repeated['occurrence_lines'], [2,4])
        self.assertEqual(repeated['successor_values'], ['River','Hill'])

    def test_same_name_in_different_roles_is_not_one_anchor(self):
        raw = ("[1] toolbar 'Harbor'\n  [2] button 'Release'\n  [3] button 'River'\n"
               "  [4] link 'Release'\n  [5] link 'Hill'\n[6] heading 'End'\n")
        source, = project_sources([{'type':'text','value':raw}],source_graph=True)
        group, = source['graph']
        self.assertEqual(group['repeated_labels'], [])
        self.assertEqual(group['peers'][0]['role'], 'button')
        self.assertEqual(group['peers'][2]['role'], 'link')

    def test_same_name_in_different_scopes_stays_separate(self):
        raw = ("[1] toolbar 'Harbor'\n  [2] button 'Release'\n  [3] button 'River'\n"
               "[4] toolbar 'Mountain'\n  [5] button 'Release'\n  [6] button 'Hill'\n[7] heading 'End'\n")
        source, = project_sources([{'type':'text','value':raw}],source_graph=True)
        first, second = source['graph']
        self.assertEqual(first['parent']['line'],1)
        self.assertEqual(second['parent']['line'],4)
        self.assertEqual(first['peers'][0]['successor']['name'],'River')
        self.assertEqual(second['peers'][0]['successor']['name'],'Hill')
        self.assertEqual(first['repeated_labels'],[])
        self.assertEqual(second['repeated_labels'],[])

    def test_opaque_intervening_group_prevents_invented_neighbor(self):
        raw = ("[1] toolbar 'Harbor'\n  [2] button 'Release'\n  [3] group 'Other'\n"
               "    [4] button 'Hidden'\n  [5] button 'River'\n[6] heading 'End'\n")
        source, = project_sources([{'type':'text','value':raw}],source_graph=True)
        group, = source['graph']
        self.assertEqual(group['peers'][0]['successor'],{'state':'unresolved'})
        self.assertEqual(group['peers'][1]['successor'],{'state':'unresolved'})

    def test_clipped_repeat_keeps_unresolved_occurrence(self):
        raw = ("[1] toolbar 'Harbor'\n  [2] button 'Release'\n  [3] button 'River'\n"
               "  [4] button 'Release'\n  [5] button 'Clipp")
        source, = project_sources([{'type':'text','value':raw}],source_graph=True)
        group, = source['graph']
        repeated, = group['repeated_labels']
        self.assertEqual(repeated['successor_values'],['River'])
        self.assertEqual(repeated['unresolved_occurrence_lines'],[4])
        self.assertEqual(group['peers'][-1]['successor'],{'state':'unresolved'})

    def test_virtualized_scope_never_proves_adjacency_or_end(self):
        raw = ("[1] toolbar 'Harbor', virtualized=true\n  [2] button 'Release'\n"
               "  [3] button 'River'\n[4] heading 'End'\n")
        source, = project_sources([{'type':'text','value':raw}],source_graph=True)
        group, = source['graph']
        self.assertEqual(group['peers'][0]['successor'],{'state':'unresolved'})
        self.assertEqual(group['peers'][1]['successor'],{'state':'unresolved'})

    def test_graph_quote_is_not_accepted_as_source_evidence(self):
        row = {'question_id':'g','question_text':'Which?', 'prompt_messages':[],
               'memory_context':[{'type':'text','value':r"[1] button 'Cr\u00e8me'\n"}]}
        truth = {'answer_kind':'single_value','accepted_answers':['Crème']}
        literal = {'response_raw':r'\boxed{Crème}'+"\nQUOTE: [1] button 'Crème'",'finish_reason':'stop'}
        invented = {'response_raw':r'\boxed{Crème}'+'\nQUOTE: "state": "end"','finish_reason':'stop'}
        self.assertTrue(classify(row,truth,literal,self.variant)['quote_exact'])
        self.assertFalse(classify(row,truth,invented,self.variant)['quote_exact'])

    def test_virtualized_peer_count_does_not_claim_complete_membership(self):
        raw = "[1] toolbar 'Harbor', virtualized=true\n  [2] button 'Birch'\n[3] heading 'End'\n"
        source, = project_sources([{'type':'text','value':raw}],source_graph=True)
        group, = source['graph']
        self.assertFalse(group['membership_complete'])

    def test_frozen_graph_retains_raw_offsets_and_registers_treatment(self):
        raw = r"[1] toolbar 'Harbor'\n\t[2] button 'Cr\u00e8me'\n[3] heading 'End'\n"
        with TemporaryDirectory() as tmp:
            root=Path(tmp)
            (root/'rows.jsonl').write_text(json.dumps({'question_id':'g','question_text':'Which?',
                'prompt_messages':[],'memory_context':[{'type':'text','value':raw}]})+'\n')
            (root/'truth.json').write_text(json.dumps({'cases':{'g':{
                'accepted_answers':['Crème'],'answer_kind':'single_value'}}}))
            freeze(root/'run',root/'rows.jsonl',root/'truth.json','local','http://127.0.0.1:9999/v1',variants=[self.variant])
            source, = json.loads((root/'run/projection.json').read_text())['g']
            peer, = source['graph'][0]['peers']
            self.assertEqual(raw[peer['raw_range'][0]:peer['raw_range'][1]],r"[2] button 'Cr\u00e8me'")
            plan=json.loads((root/'run/registration.json').read_text())
            self.assertEqual(plan['variant_treatments'][self.variant],
                             ['source_graph','typed_source_locations','quote_channel_separation','reasoning_instruction'])
