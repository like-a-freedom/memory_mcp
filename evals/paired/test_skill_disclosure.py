"""Workflow disclosure through the public host and message contracts."""
import unittest
import json
import sys
from pathlib import Path
from tempfile import TemporaryDirectory
from evals.paired.skill_workflow import SkillHost, create_messages


class SkillDisclosureTests(unittest.TestCase):
    def test_selected_live_schema_is_returned_exactly_without_calling_tool(self):
        schema = {'type':'object','required':['query'],'properties':{'query':{'type':'string'}}}
        calls = []
        host = SkillHost('memory-mcp',{}, {'mcp__memory__assemble_context':lambda args:calls.append(args)},
                         tool_schemas={'mcp__memory__assemble_context':schema})
        response = host.dispatch({'action':'describe_tool','name':'mcp__memory__assemble_context'})
        self.assertEqual(response, {'name':'mcp__memory__assemble_context','inputSchema':schema})
        self.assertEqual(calls, [])
        denied = host.dispatch({'action':'describe_tool','name':'mcp__memory__unknown'})
        self.assertTrue(denied['denied'])

    def test_progressive_catalog_keeps_all_tool_names_and_defers_exact_schemas(self):
        schema = {'mcp__memory__extract':{'type':'object','description':'SCHEMA_ONLY_MARKER'},
                  'mcp__memory__invalidate':{'type':'object'}}
        messages = create_messages({'prompt':'Use memory','fixture':{}},'skill',schema,
                                   workflow_protocol='progressive-completion/v1')
        text = messages[0]['content']
        self.assertIn('mcp__memory__extract',text)
        self.assertIn('mcp__memory__invalidate',text)
        self.assertNotIn('SCHEMA_ONLY_MARKER',text)
        self.assertIn('describe_tool',text)
        self.assertIn('Finish as soon as',text)

    def test_only_registered_workflow_protocol_can_be_frozen_and_verified(self):
        from evals.paired.skill_workflow import freeze,verify_plan
        with TemporaryDirectory() as directory:
            path = Path(directory)/'plan'
            plan = freeze(path,Path(sys.executable),'http://127.0.0.1:9/v1','controlled',
                          workflow_protocol='progressive-completion/v1')
            verify_plan(plan)
            self.assertEqual(plan['workflow_protocol'],'progressive-completion/v1')
            plan['workflow_protocol'] = 'unregistered'
            with self.assertRaisesRegex(ValueError,'unregistered workflow protocol'):
                verify_plan(plan)

    def test_changing_registered_mode_cannot_reinterpret_legacy_action_contract(self):
        from evals.paired.skill_workflow import freeze,verify_plan
        with TemporaryDirectory() as directory:
            plan = freeze(Path(directory)/'plan',Path(sys.executable),
                          'http://127.0.0.1:9/v1','controlled')
            plan['workflow_protocol'] = 'progressive-completion/v1'
            with self.assertRaisesRegex(ValueError,'protocol registration'):
                verify_plan(plan)

    def test_schema_catalog_cannot_omit_a_live_tool(self):
        with self.assertRaisesRegex(ValueError,'every live tool'):
            SkillHost('memory-mcp',{}, {'mcp__memory__extract':lambda args:{},
                                      'mcp__memory__invalidate':lambda args:{}},
                      tool_schemas={'mcp__memory__extract':{'type':'object'}})

class CompletionFaultTests(unittest.IsolatedAsyncioTestCase):
    async def test_completed_operations_survive_failed_final_response_without_synthetic_finish(self):
        from evals.paired.skill_workflow import SOURCE,play,grade_observed
        schemas = {'mcp__memory__ingest':{'type':'object'},'mcp__memory__extract':{'type':'object'}}
        host = SkillHost('memory-mcp',{}, {
            'mcp__memory__ingest':lambda args:{'status':'success','result':'episode:recorded'},
            'mcp__memory__extract':lambda args:{'status':'success','result':{
                'episode_id':'episode:recorded','facts':[{'fact_id':'fact:recorded'}]}}},tool_schemas=schemas)
        actions = iter([
            {'action':'call','name':'mcp__memory__ingest','arguments':SOURCE},
            {'action':'call','name':'mcp__memory__extract','arguments':{'episode_id':'episode:recorded'}},
            None])

        async def request(messages,deadline):
            action = next(actions)
            if action is None:
                raise TimeoutError('final response deadline')
            return {'choices':[{'message':{'content':json.dumps(action)},'finish_reason':'stop'}]}

        result = await play(create_messages({'prompt':'Capture source','fixture':{}},'skill',schemas,
                            workflow_protocol='progressive-completion/v1'),host,request)
        grade = grade_observed({'skill':'memory-mcp','id':1},result,host.trace)
        self.assertTrue(grade['operation_and_exact_arguments'])
        self.assertEqual(result['terminal'],'error')
        self.assertEqual(result['error_type'],'TimeoutError')
        self.assertIsNone(result['answer'])
        self.assertEqual(len(host.trace),2)


class FreshQualificationTests(unittest.TestCase):
    def test_fresh_native_cohort_has_disjoint_sources_and_time_constrained_reads(self):
        from evals.paired.skill_workflow import freeze
        with TemporaryDirectory() as directory:
            plan = freeze(Path(directory)/'plan',Path(sys.executable),
                          'http://127.0.0.1:9/v1','controlled',
                          workflow_protocol='progressive-completion/v1',case_cohort='fresh-native/v1')
            self.assertEqual(len(plan['cases']),6)
            self.assertTrue(all(c['binding']=='native' for c in plan['cases']))
            self.assertEqual({c['objective_id'] for c in plan['cases']},{1,2})
            timed = [c for c in plan['cases'] if c.get('required_selectors')]
            self.assertEqual(len(timed),2)
            self.assertTrue(all(c['source']['source_id']!='cedar-release-minutes-2026-11-20'
                                for c in plan['cases']))

    def test_fresh_capture_is_graded_against_registered_source_not_historical_cedar(self):
        from evals.paired.skill_workflow import grade_observed
        source = {'source_type':'note','source_id':'fresh-maple','t_ref':'2026-11-10T10:00:00Z',
                  'content':'Decision: Maple retains records for 19 days.'}
        case = {'skill':'memory-mcp','id':101,'objective_id':1,'source':source}
        trace = [
            {'request':{'action':'call','name':'mcp__memory__ingest','arguments':source},
             'response':{'status':'success','result':'episode:fresh'}},
            {'request':{'action':'call','name':'mcp__memory__extract','arguments':{'episode_id':'episode:fresh'}},
             'response':{'status':'success','result':{'episode_id':'episode:fresh','facts':[{'fact_id':'fact:fresh'}]}}}]
        result = grade_observed(case,{'terminal':'answered'},trace)
        self.assertTrue(result['operation_and_exact_arguments'])
        trace[0]['request']['arguments'] = {**source,'source_id':'wrong-source'}
        self.assertFalse(grade_observed(case,{'terminal':'answered'},trace)['operation_and_exact_arguments'])

    def test_time_constrained_recall_rejects_a_dropped_known_boundary(self):
        from evals.paired.skill_workflow import grade_observed
        source = {'source_id':'juniper-fresh','content':'Decision: Juniper updates Thursday.'}
        case = {'skill':'memory-mcp','id':103,'objective_id':2,'source':source,
                'required_selectors':{'valid_at':'2026-11-13T12:00:00Z','known_at':'2026-11-27T18:00:00Z'}}
        item = {'fact_id':'fact:fresh','source_episode':'episode:fresh',
                'provenance':{'source_id':source['source_id']},'citation_context':source['content']}
        trace = [
            {'request':{'action':'call','name':'mcp__memory__assemble_context',
                        'arguments':{'query':'Juniper', 'valid_at':'2026-11-13T12:00:00Z'}},
             'response':{'status':'success','result':[item]}},
            {'request':{'action':'call','name':'mcp__memory__explain',
                        'arguments':{'context_items':json.dumps([{'fact_id':'fact:fresh'}])}},
             'response':{'status':'success','result':[item]}}]
        result = grade_observed(case,{'terminal':'answered'},trace)
        self.assertTrue(result['source_binding_observed'])
        self.assertFalse(result['operation_and_exact_arguments'])

    def test_fresh_case_registration_cannot_be_changed_after_freeze(self):
        from evals.paired.skill_workflow import freeze, verify_plan
        with TemporaryDirectory() as directory:
            plan = freeze(Path(directory)/'plan',Path(sys.executable),
                          'http://127.0.0.1:9/v1','controlled',case_cohort='fresh-native/v1')
            verify_plan(plan)
            plan['cases'][0]['source']['source_id'] = 'unregistered-source'
            with self.assertRaisesRegex(ValueError,'cohort registration'):
                verify_plan(plan)

    def test_removed_cohort_label_cannot_bypass_fresh_case_identity(self):
        from evals.paired.skill_workflow import freeze, verify_plan
        with TemporaryDirectory() as directory:
            plan = freeze(Path(directory)/'plan',Path(sys.executable),
                          'http://127.0.0.1:9/v1','controlled',case_cohort='fresh-native/v1')
            plan['cases'][0]['source']['source_id'] = 'unregistered-source'
            del plan['case_cohort']
            with self.assertRaisesRegex(ValueError,'cohort registration'):
                verify_plan(plan)

    def test_fresh_recall_cannot_accept_a_foreign_explain_item_with_copied_text(self):
        from evals.paired.skill_workflow import grade_observed
        source = {'source_id':'willow-fresh','content':'Decision: Willow updates Thursday.'}
        case = {'skill':'memory-mcp','id':102,'objective_id':2,'source':source}
        item = {'fact_id':'fact:selected','source_episode':'episode:selected',
                'provenance':{'source_id':source['source_id']},'citation_context':source['content']}
        trace = [
            {'request':{'action':'call','name':'mcp__memory__assemble_context','arguments':{'query':'Willow'}},
             'response':{'status':'success','result':[item]}},
            {'request':{'action':'call','name':'mcp__memory__explain',
                        'arguments':{'context_items':json.dumps([{'fact_id':'fact:selected'}])}},
             'response':{'status':'success','result':[{**item,'fact_id':'fact:foreign','source_episode':'episode:foreign'}]}}]
        grade = grade_observed(case,{'terminal':'answered'},trace)
        self.assertFalse(grade['operation_and_exact_arguments'])
        self.assertFalse(grade['source_binding_observed'])

    def test_fresh_recall_rejects_an_error_envelope_even_with_matching_source(self):
        from evals.paired.skill_workflow import grade_observed
        source = {'source_id':'willow-fresh','content':'Decision: Willow updates Thursday.'}
        case = {'skill':'memory-mcp','id':102,'objective_id':2,'source':source}
        item = {'fact_id':'fact:selected','source_episode':'episode:selected',
                'provenance':{'source_id':source['source_id']},'citation_context':source['content']}
        trace = [
            {'request':{'action':'call','name':'mcp__memory__assemble_context','arguments':{'query':'Willow'}},
             'response':{'status':'success','result':[item]}},
            {'request':{'action':'call','name':'mcp__memory__explain',
                        'arguments':{'context_items':json.dumps([{'fact_id':'fact:selected'}])}},
             'response':{'status':'error','result':[item]}}]
        self.assertFalse(grade_observed(case,{'terminal':'answered'},trace)['operation_and_exact_arguments'])

    def test_time_constrained_recall_rejects_a_different_returned_snapshot(self):
        from evals.paired.skill_workflow import grade_observed
        source = {'source_id':'juniper-fresh','content':'Decision: Juniper updates Thursday.'}
        required = {'valid_at':'2026-11-13T12:00:00Z','known_at':'2099-11-27T18:00:00Z'}
        case = {'skill':'memory-mcp','id':103,'objective_id':2,'source':source,'required_selectors':required}
        item = {'fact_id':'fact:selected','source_episode':'episode:selected',
                'provenance':{'source_id':source['source_id']},'citation_context':source['content']}
        trace = [
            {'request':{'action':'call','name':'mcp__memory__assemble_context',
                        'arguments':{'query':'Juniper',**required}},
             'response':{'status':'partial','result':[item],
                         'context':{'snapshot':{**required,'known_at':'2026-01-01T00:00:00Z'}}}},
            {'request':{'action':'call','name':'mcp__memory__explain',
                        'arguments':{'context_items':json.dumps([{'fact_id':'fact:selected'}])}},
             'response':{'status':'success','result':[item]}}]
        self.assertFalse(grade_observed(case,{'terminal':'answered'},trace)['operation_and_exact_arguments'])


if __name__ == '__main__':
    unittest.main()
