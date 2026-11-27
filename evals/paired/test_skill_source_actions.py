"""Authorized source actions through the public workflow grader."""
import json
from contextlib import asynccontextmanager,contextmanager
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from unittest.mock import patch
import unittest

from evals.paired.skill_workflow import grade_observed


PROTOCOL = 'memory-mcp-skill-source-actions/v3'


class SourceActionTests(unittest.TestCase):
    def setUp(self):
        self.source = {'source_id':'alder-reviewed','content':'Decision: Alder retains 23 days.'}
        self.case = {'skill':'memory-mcp','id':201,'objective_id':2,
                     'operation_protocol':PROTOCOL,'source':self.source}
        self.item = {'fact_id':'episode_fallback:episode:alder','source_episode':'episode:alder',
                     'provenance':{'source_id':'alder-reviewed','episode_fallback':True,
                                   'source_episode':'episode:alder'},'content':self.source['content']}
        self.read = {'status':'success','result':[self.item]}
        self.explain = {'status':'success','result':[{**self.item,
                        'citation_context':self.source['content'],'citation_context_exact':True,
                        'citation_context_covers_entire_source':True,'quote_verified':False}]}
        self.trace = [
            {'request':{'action':'call','name':'mcp__memory__assemble_context',
                        'arguments':{'query':'Alder retention'}},'response':self.read},
            {'request':{'action':'call','name':'mcp__memory__explain',
                        'arguments':{'context_items':json.dumps([self.item])}},'response':self.explain}]

    def capture_trace(self, extracted):
        source = {'source_type':'conversation','source_id':'alder-reviewed',
                  't_ref':'2026-11-14T10:30:00Z','content':self.source['content']}
        self.case.update(objective_id=1,source=source)
        return [
            {'request':{'action':'call','name':'mcp__memory__ingest','arguments':source},
             'response':{'status':'success','result':'episode:alder'}},
            {'request':{'action':'call','name':'mcp__memory__extract',
                        'arguments':{'episode_id':'episode:alder'}},'response':extracted}]

    def test_failed_extraction_receipt_cannot_pass_capture_operation(self):
        trace = self.capture_trace({'status':'error','result':{'episode_id':'episode:alder','facts':[]}})

        grade = grade_observed(self.case,{'terminal':'answered'},trace)

        self.assertFalse(grade['operation_and_exact_arguments'])
        self.assertFalse(grade['source_binding_observed'])
        self.assertEqual(grade['pending_vs_completed_oracle'],'pending')

    def test_success_receipt_for_another_episode_cannot_pass_capture(self):
        trace = self.capture_trace({'status':'success','result':{'episode_id':'episode:foreign','facts':[]}})
        grade = grade_observed(self.case,{'terminal':'answered'},trace)
        self.assertFalse(grade['operation_and_exact_arguments'])
        self.assertFalse(grade['source_binding_observed'])

    def test_authorized_fallback_readback_does_not_upgrade_episode_only_capture(self):
        trace = self.capture_trace({'status':'success','result':{'episode_id':'episode:alder','facts':[]}})
        trace.extend(self.trace)

        grade = grade_observed(self.case,{'terminal':'answered'},trace)

        self.assertTrue(grade['operation_and_exact_arguments'])
        self.assertTrue(grade['source_sufficient'])
        self.assertFalse(grade['canonical_fact_verified'])
        self.assertEqual(grade['pending_vs_completed_oracle'],'episode-only')

    def test_bound_episode_fallback_satisfies_source_action_without_canonical_fact(self):
        grade = grade_observed(self.case,{'terminal':'answered'},self.trace)

        self.assertTrue(grade['operation_and_exact_arguments'])
        self.assertTrue(grade['source_sufficient'])
        self.assertFalse(grade['canonical_fact_verified'])

    def test_minimal_api_selection_binds_without_incidental_ranking_fields(self):
        self.case['operation_protocol'] = 'memory-mcp-skill-source-actions/v4'
        self.trace[1]['request']['arguments']['context_items'] = json.dumps([{
            'fact_id':self.item['fact_id'], 'source_episode':'episode:alder',
            'content':self.source['content']}])

        grade = grade_observed(self.case,{'terminal':'answered'},self.trace)

        self.assertTrue(grade['operation_and_exact_arguments'])
        self.assertTrue(grade['source_sufficient'])
        self.assertFalse(grade['canonical_fact_verified'])

    def test_episode_source_read_is_sufficient_without_claiming_fact_verification(self):
        self.case['operation_protocol'] = 'memory-mcp-skill-source-actions/v4'
        self.item['fact_id'] = 'fact:alder'
        self.item['provenance']['episode_fallback'] = False
        self.trace[1]['request']['arguments']['context_items'] = json.dumps(['episode:alder'])
        self.explain['result'][0].pop('fact_id')

        grade = grade_observed(self.case,{'terminal':'answered'},self.trace)

        self.assertTrue(grade['source_sufficient'])
        self.assertTrue(grade['operation_and_exact_arguments'])
        self.assertFalse(grade['canonical_fact_verified'])

    def test_source_only_object_binds_without_verifying_canonical_fact(self):
        self.case['operation_protocol'] = 'memory-mcp-skill-source-actions/v4'
        self.item['fact_id'] = 'fact:alder'
        self.item['provenance']['episode_fallback'] = False
        self.trace[1]['request']['arguments']['context_items'] = json.dumps([
            {'source_episode':'episode:alder'}])
        self.explain['result'][0].pop('fact_id')

        grade = grade_observed(self.case,{'terminal':'answered'},self.trace)

        self.assertTrue(grade['source_sufficient'])
        self.assertTrue(grade['operation_and_exact_arguments'])
        self.assertFalse(grade['canonical_fact_verified'])

    def test_matching_fact_without_required_episode_cannot_bind(self):
        self.case['operation_protocol'] = 'memory-mcp-skill-source-actions/v4'
        self.trace[1]['request']['arguments']['context_items'] = json.dumps([
            {'fact_id':self.item['fact_id']}])

        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['source_sufficient'])

    def test_episode_string_whitespace_is_normalized_like_public_parser(self):
        self.case['operation_protocol'] = 'memory-mcp-skill-source-actions/v4'
        self.trace[1]['request']['arguments']['context_items'] = json.dumps(['  episode:alder\n'])
        self.explain['result'][0].pop('fact_id')

        self.assertTrue(grade_observed(self.case,{'terminal':'answered'},self.trace)['source_sufficient'])

    def test_episode_object_whitespace_is_normalized_like_public_parser(self):
        self.case['operation_protocol'] = 'memory-mcp-skill-source-actions/v4'
        self.trace[1]['request']['arguments']['context_items'] = json.dumps([
            {'source_episode':'  episode:alder\n'}])
        self.explain['result'][0].pop('fact_id')

        self.assertTrue(grade_observed(self.case,{'terminal':'answered'},self.trace)['source_sufficient'])

    def test_invalid_unselected_entry_cannot_be_hidden_by_item_offset(self):
        self.case['operation_protocol'] = 'memory-mcp-skill-source-actions/v4'
        self.trace[1]['request']['arguments'].update(
            context_items=json.dumps([self.item,{'fact_id':'fact:foreign'}]), item_offset=0)

        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['source_sufficient'])

    def test_legacy_alias_cannot_bypass_strict_public_parser(self):
        self.case['operation_protocol'] = 'memory-mcp-skill-source-actions/v4'
        self.trace[1]['request']['arguments']['context_items'] = json.dumps([
            {**self.item,'factId':self.item['fact_id']}])

        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['source_sufficient'])

    def test_empty_optional_text_fields_keep_valid_identity_selection(self):
        self.case['operation_protocol'] = 'memory-mcp-skill-source-actions/v4'
        self.trace[1]['request']['arguments']['context_items'] = json.dumps([
            {'source_episode':'episode:alder','fact_id':self.item['fact_id'],
             'content':'','quote':''}])

        self.assertTrue(grade_observed(self.case,{'terminal':'answered'},self.trace)['source_sufficient'])

    def test_non_string_optional_fields_follow_public_parser_defaults(self):
        self.case['operation_protocol'] = 'memory-mcp-skill-source-actions/v4'
        self.trace[1]['request']['arguments']['context_items'] = json.dumps([
            {'source_episode':'episode:alder','fact_id':17,'content':23,'quote':[] }])
        self.explain['result'][0].pop('fact_id')

        grade = grade_observed(self.case,{'terminal':'answered'},self.trace)

        self.assertTrue(grade['source_sufficient'])
        self.assertFalse(grade['canonical_fact_verified'])

    def test_minimal_selection_of_foreign_fact_cannot_bind(self):
        self.case['operation_protocol'] = 'memory-mcp-skill-source-actions/v4'
        self.trace[1]['request']['arguments']['context_items'] = json.dumps([{'fact_id':'fact:foreign'}])
        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['source_sufficient'])

    def test_episode_selection_cannot_rescue_explanation_of_a_foreign_fact(self):
        self.case['operation_protocol'] = 'memory-mcp-skill-source-actions/v4'
        self.trace[1]['request']['arguments']['context_items'] = json.dumps(['episode:alder'])
        self.explain['result'][0]['fact_id'] = 'fact:foreign'
        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['source_sufficient'])

    def test_unselected_matching_item_cannot_rescue_foreign_selected_offset(self):
        self.case['operation_protocol'] = 'memory-mcp-skill-source-actions/v4'
        self.trace[1]['request']['arguments'].update(
            context_items=json.dumps([self.item,{'fact_id':'fact:foreign'}]), item_offset=1)
        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['source_sufficient'])

    def test_changed_content_on_matching_identity_cannot_bind(self):
        self.case['operation_protocol'] = 'memory-mcp-skill-source-actions/v4'
        self.trace[1]['request']['arguments']['context_items'] = json.dumps([
            {'fact_id':self.item['fact_id'],'source_episode':'episode:alder',
             'content':'Decision: Alder retains 99 years.'}])
        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['source_sufficient'])

    def test_previous_contract_keeps_its_historical_minimal_selection_failure(self):
        self.trace[1]['request']['arguments']['context_items'] = json.dumps([{'fact_id':self.item['fact_id']}])
        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['source_sufficient'])

    def test_new_selection_cohort_is_registered_before_capture(self):
        from evals.paired.skill_workflow import freeze,verify_plan
        with TemporaryDirectory() as directory:
            plan = freeze(Path(directory)/'plan',Path(sys.executable),'http://127.0.0.1:9/v1',
                          'controlled',workflow_protocol='progressive-completion/v1',
                          case_cohort='source-actions-native/v2')
            verify_plan(plan)
            self.assertEqual(plan['operation_protocol'],'memory-mcp-skill-source-actions/v4')
            self.assertEqual(len(plan['cases']),6)

    def test_correct_source_cannot_rescue_a_different_returned_snapshot(self):
        selectors = {'valid_at':'2026-11-12T10:00:00Z','known_at':'2099-11-27T18:00:00Z'}
        self.case['required_selectors'] = selectors
        self.trace[0]['request']['arguments'].update(selectors)
        self.read['context'] = {'snapshot':{**selectors,'valid_at':'2026-11-13T10:00:00Z'}}

        grade = grade_observed(self.case,{'terminal':'answered'},self.trace)

        self.assertFalse(grade['operation_and_exact_arguments'])

    def test_copied_text_on_a_foreign_episode_cannot_bind(self):
        self.explain['result'][0]['source_episode'] = 'episode:foreign'
        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['operation_and_exact_arguments'])

    def test_copied_text_on_a_foreign_fact_cannot_bind(self):
        self.explain['result'][0]['fact_id'] = 'fact:foreign'
        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['operation_and_exact_arguments'])

    def test_copied_text_on_a_foreign_source_cannot_bind(self):
        self.explain['result'][0]['provenance'] = {
            **self.explain['result'][0]['provenance'],'source_id':'foreign-source'}
        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['operation_and_exact_arguments'])

    def test_inexact_source_context_does_not_establish_source_sufficiency(self):
        self.explain['result'][0]['citation_context_exact'] = False
        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['operation_and_exact_arguments'])

    def test_error_response_cannot_be_empty_success(self):
        self.explain['status'] = 'error'
        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['operation_and_exact_arguments'])

    def test_extra_mutation_invalidates_recall_action(self):
        self.trace.append({'request':{'action':'call','name':'mcp__memory__resolve',
                                     'arguments':{'canonical_name':'Alder'}},
                           'response':{'status':'success','result':{'entity_id':'entity:alder'}}})
        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['operation_and_exact_arguments'])

    def test_dropped_request_time_selector_fails_even_when_returned_snapshot_matches(self):
        selectors = {'valid_at':'2026-11-12T10:00:00Z','known_at':'2099-11-27T18:00:00Z'}
        self.case['required_selectors'] = selectors
        self.trace[0]['request']['arguments'].update({'valid_at':selectors['valid_at']})
        self.read['context'] = {'snapshot':selectors}
        self.assertFalse(grade_observed(self.case,{'terminal':'answered'},self.trace)['operation_and_exact_arguments'])

    def test_actual_canonical_fact_coverage_is_reported_separately(self):
        self.item['fact_id'] = 'fact:alder'
        self.item['provenance']['episode_fallback'] = False
        self.explain['result'][0]['fact_id'] = 'fact:alder'
        self.trace[1]['request']['arguments']['context_items'] = json.dumps([self.item])

        grade = grade_observed(self.case,{'terminal':'answered'},self.trace)

        self.assertTrue(grade['operation_and_exact_arguments'])
        self.assertTrue(grade['canonical_fact_verified'])

    def test_unknown_operation_protocol_cannot_fall_back_to_legacy(self):
        self.case['operation_protocol'] = 'unregistered'
        with self.assertRaisesRegex(ValueError,'operation protocol'):
            grade_observed(self.case,{'terminal':'answered'},self.trace)

    def test_source_action_freeze_registers_distinct_cases_and_scorer(self):
        from evals.paired.skill_workflow import freeze,verify_plan
        with TemporaryDirectory() as directory:
            plan = freeze(Path(directory)/'plan',Path(sys.executable),'http://127.0.0.1:9/v1',
                          'controlled',workflow_protocol='progressive-completion/v1',
                          case_cohort='source-actions-native/v1')
            verify_plan(plan)

            self.assertEqual(plan['operation_protocol'],PROTOCOL)
            self.assertEqual(len(plan['cases']),6)
            self.assertIn('source_action_scorer_sha256',plan)
            self.assertTrue(all(case['operation_protocol']==PROTOCOL for case in plan['cases']))

    def test_registered_candidate_skill_cannot_be_replaced_before_capture(self):
        from evals.paired.skill_workflow import freeze,verify_plan
        with TemporaryDirectory() as directory:
            plan = freeze(Path(directory)/'plan',Path(sys.executable),'http://127.0.0.1:9/v1',
                          'controlled',workflow_protocol='progressive-completion/v1',
                          case_cohort='source-actions-native/v1')
            plan['skills']['memory-mcp']['current']['SKILL.md'] += '\nUnregistered instruction'
            with self.assertRaisesRegex(ValueError,'skill registration'):
                verify_plan(plan)

    def test_authorized_capture_readback_and_explain_preserve_verified_capture(self):
        source = {'source_type':'conversation','source_id':'birch-review',
                  't_ref':'2026-11-12T09:00:00Z','content':'Decision: Birch retains 31 days.'}
        case = {**self.case,'objective_id':1,'source':source}
        item = {'fact_id':'fact:birch','source_episode':'episode:birch',
                'provenance':{'source_id':source['source_id']},'content':source['content']}
        trace = [
            {'request':{'action':'call','name':'mcp__memory__ingest','arguments':source},
             'response':{'status':'success','result':'episode:birch'}},
            {'request':{'action':'call','name':'mcp__memory__extract',
                        'arguments':{'episode_id':'episode:birch'}},
             'response':{'status':'success','result':{'episode_id':'episode:birch',
                         'facts':[{'fact_id':'fact:birch','type':'note'}]}}},
            {'request':{'action':'call','name':'mcp__memory__assemble_context',
                        'arguments':{'query':'Birch retention'}},'response':{'status':'success','result':[item]}},
            {'request':{'action':'call','name':'mcp__memory__explain',
                        'arguments':{'context_items':json.dumps([item])}},
             'response':{'status':'success','result':[{**item,'citation_context_exact':True,
                          'citation_context':source['content']}]}}]

        grade = grade_observed(case,{'terminal':'answered'},trace)

        self.assertTrue(grade['operation_and_exact_arguments'])
        self.assertEqual(grade['pending_vs_completed_oracle'],'verified')


class SourceActionCaptureTests(unittest.IsolatedAsyncioTestCase):
    async def test_capture_labels_its_actual_registered_operation_protocol(self):
        from evals.paired.skill_workflow import freeze,capture,SkillHost

        @contextmanager
        def host_binding(*args):
            yield SkillHost('memory-cli',{},{}),'controlled skill',None

        async def completion(**kwargs):
            return {'choices':[{'finish_reason':'stop','message':{
                'content':'{"action":"finish","answer":"No operation was performed."}'}}]}

        @asynccontextmanager
        async def client_binding(*args):
            yield SimpleNamespace(chat=SimpleNamespace(completions=SimpleNamespace(create=completion)))

        with TemporaryDirectory() as directory:
            path = Path(directory)/'plan'
            freeze(path,Path(sys.executable),'http://127.0.0.1:9/v1','controlled',
                   workflow_protocol='progressive-completion/v1',case_cohort='source-actions-native/v1')
            with patch('evals.paired.skill_workflow.trial_host',host_binding),patch(
                       'evals.paired.skill_workflow.local_async_client',client_binding):
                await capture(path)
            summary = json.loads((path/'capture/summary.json').read_text())

            self.assertEqual(summary['operation_protocol'],PROTOCOL)
            self.assertEqual(summary['scheduled'],12)
            self.assertFalse(any(row['grade']['operation_and_exact_arguments'] for row in summary['rows']))


if __name__ == '__main__':
    unittest.main()
