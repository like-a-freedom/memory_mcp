"""Controlled subprocess integration at the public MCP/agent boundary."""
import json
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from evals.paired.memory_quality_diagnostic import MCP
from evals.paired.skill_workflow import SkillHost, grade_observed, play


SERVER = '''import json,sys
for line in sys.stdin:
 request=json.loads(line)
 if 'id' not in request:continue
 response={'jsonrpc':'2.0','id':request['id'],'result':{}}
 if request['method']=='tools/call':
  response={'jsonrpc':'2.0','id':request['id'],'error':{'code':-32602,'message':'source_episode required'}}
 print(json.dumps(response),flush=True)
'''


async def play_controlled_server(server):
    actual_popen = subprocess.Popen
    def controlled_process(argv, **kwargs):
        return actual_popen([sys.executable,'-u','-c',server], **kwargs)

    with TemporaryDirectory() as directory:
        with patch('evals.paired.memory_quality_diagnostic.subprocess.Popen',side_effect=controlled_process):
            binding = MCP({'product_binary':sys.executable,'launch_env':{}},Path(directory))
        try:
            host = SkillHost('memory-mcp',{}, {'mcp__memory__explain':lambda args:binding.tool('explain',args)})
            actions = iter([
                {'action':'call','name':'mcp__memory__explain','arguments':{}},
                {'action':'finish','answer':'No verification completed.'}])
            async def request(messages,deadline):
                return {'choices':[{'message':{'content':json.dumps(next(actions))},'finish_reason':'stop'}]}
            return await play([],host,request),host.trace
        finally:
            binding.close()


class McpRecoveryIntegrationTests(unittest.IsolatedAsyncioTestCase):
    async def test_invalid_tool_content_json_is_terminal(self):
        malformed = SERVER.replace(
            "response={'jsonrpc':'2.0','id':request['id'],'error':{'code':-32602,'message':'source_episode required'}}",
            "response={'jsonrpc':'2.0','id':request['id'],'result':{'content':[{'type':'text','text':'not-json'}]}}")

        result,trace = await play_controlled_server(malformed)

        self.assertEqual(result['terminal'],'error')
        self.assertEqual(result['error_type'],'RuntimeError')
        self.assertIsNone(result['answer'])
        self.assertEqual(trace[0]['response']['host_error'],'RuntimeError')

    async def test_correlated_envelope_without_result_or_error_is_terminal(self):
        missing = SERVER.replace(
            "response={'jsonrpc':'2.0','id':request['id'],'error':{'code':-32602,'message':'source_episode required'}}",
            "response={'jsonrpc':'2.0','id':request['id']}")

        result,trace = await play_controlled_server(missing)

        self.assertEqual(result['terminal'],'error')
        self.assertEqual(result['error_type'],'RuntimeError')
        self.assertIsNone(result['answer'])
        self.assertEqual(trace[0]['response']['host_error'],'RuntimeError')

    async def test_invalid_transport_json_is_terminal_not_an_argument_refusal(self):
        broken = SERVER.replace(
            "  response={'jsonrpc':'2.0','id':request['id'],'error':{'code':-32602,'message':'source_episode required'}}",
            "  print('not-json',flush=True)\n  continue")

        result,trace = await play_controlled_server(broken)

        self.assertEqual(result['terminal'],'error')
        self.assertEqual(result['error_type'],'RuntimeError')
        self.assertIsNone(result['answer'])
        self.assertEqual(trace[0]['response']['host_error'],'RuntimeError')

    async def test_initialize_error_is_terminal_and_releases_subprocess_resources(self):
        actual_popen = subprocess.Popen
        children = []
        initialization_error = SERVER.replace("if request['method']=='tools/call':", "if request['method']=='initialize':")
        def controlled_process(argv, **kwargs):
            child = actual_popen([sys.executable,'-u','-c',initialization_error], **kwargs)
            children.append(child)
            return child

        with TemporaryDirectory() as directory:
            with patch('evals.paired.memory_quality_diagnostic.subprocess.Popen',side_effect=controlled_process):
                with self.assertRaises(RuntimeError) as raised:
                    MCP({'product_binary':sys.executable,'launch_env':{}},Path(directory))

            self.assertIs(type(raised.exception),RuntimeError)
            self.assertIsNotNone(children[0].poll())
            self.assertTrue(children[0].stdin.closed)
            self.assertTrue(children[0].stdout.closed)

    async def test_process_exit_remains_terminal_without_synthetic_finish(self):
        actual_popen = subprocess.Popen
        exiting = SERVER.replace("if request['method']=='tools/call':", "if request['method']=='tools/call':\n  sys.exit(0)")
        def controlled_process(argv, **kwargs):
            return actual_popen([sys.executable,'-u','-c',exiting], **kwargs)

        with TemporaryDirectory() as directory:
            with patch('evals.paired.memory_quality_diagnostic.subprocess.Popen',side_effect=controlled_process):
                binding = MCP({'product_binary':sys.executable,'launch_env':{}},Path(directory))
            try:
                host = SkillHost('memory-mcp',{}, {'mcp__memory__explain':lambda args:binding.tool('explain',args)})
                async def request(messages,deadline):
                    return {'choices':[{'message':{'content':json.dumps({
                        'action':'call','name':'mcp__memory__explain','arguments':{}})},'finish_reason':'stop'}]}

                result = await play([],host,request)

                self.assertEqual(result['terminal'],'error')
                self.assertEqual(result['error'],'MCP process exited')
                self.assertIsNone(result['answer'])
                self.assertEqual(host.trace[0]['response']['host_error'],'RuntimeError')
            finally:
                binding.close()

    async def test_foreign_rpc_error_cannot_replace_correlated_success(self):
        actual_popen = subprocess.Popen
        foreign = SERVER.replace(
            "response={'jsonrpc':'2.0','id':request['id'],'error':{'code':-32602,'message':'source_episode required'}}",
            "print(json.dumps({'jsonrpc':'2.0','id':999,'error':{'code':-32602,'message':'foreign error'}}),flush=True)\n  response={'jsonrpc':'2.0','id':request['id'],'result':{'structuredContent':{'status':'success','result':[]}}}")
        def controlled_process(argv, **kwargs):
            return actual_popen([sys.executable,'-u','-c',foreign], **kwargs)

        with TemporaryDirectory() as directory:
            with patch('evals.paired.memory_quality_diagnostic.subprocess.Popen',side_effect=controlled_process):
                binding = MCP({'product_binary':sys.executable,'launch_env':{}},Path(directory))
            try:
                host = SkillHost('memory-mcp',{}, {'mcp__memory__explain':lambda args:binding.tool('explain',args)})

                response = host.dispatch({'action':'call','name':'mcp__memory__explain','arguments':{}})

                self.assertEqual(response,{'status':'success','result':[]})
                receipts = [json.loads(line)for line in (Path(directory)/'mcp.jsonl').read_text().splitlines()]
                self.assertEqual(receipts[-1]['response']['id'],3)
                self.assertNotIn('error',receipts[-1]['response'])
            finally:
                binding.close()

    async def test_malformed_rpc_error_is_terminal_instead_of_recoverable_tool_data(self):
        actual_popen = subprocess.Popen
        malformed = SERVER.replace("{'code':-32602,'message':'source_episode required'}", "'not an RPC error object'")
        def controlled_process(argv, **kwargs):
            return actual_popen([sys.executable,'-u','-c',malformed], **kwargs)

        with TemporaryDirectory() as directory:
            with patch('evals.paired.memory_quality_diagnostic.subprocess.Popen',side_effect=controlled_process):
                binding = MCP({'product_binary':sys.executable,'launch_env':{}},Path(directory))
            try:
                host = SkillHost('memory-mcp',{}, {'mcp__memory__explain':lambda args:binding.tool('explain',args)})
                actions = iter([
                    {'action':'call','name':'mcp__memory__explain','arguments':{}},
                    {'action':'finish','answer':'Not verified.'}])
                async def request(messages,deadline):
                    return {'choices':[{'message':{'content':json.dumps(next(actions))},'finish_reason':'stop'}]}

                result = await play([],host,request)

                self.assertEqual(result['terminal'],'error')
                self.assertEqual(result['error_type'],'RuntimeError')
                self.assertIsNone(result['answer'])
            finally:
                binding.close()

    async def test_close_releases_both_subprocess_pipes(self):
        actual_popen = subprocess.Popen
        children = []
        def controlled_process(argv, **kwargs):
            child = actual_popen([sys.executable,'-u','-c',SERVER], **kwargs)
            children.append(child)
            return child

        with TemporaryDirectory() as directory:
            with patch('evals.paired.memory_quality_diagnostic.subprocess.Popen',side_effect=controlled_process):
                binding = MCP({'product_binary':sys.executable,'launch_env':{}},Path(directory))
            try:
                binding.close()
                self.assertTrue(children[0].stdin.closed)
                self.assertTrue(children[0].stdout.closed)
            finally:
                children[0].stdin.close()
                children[0].stdout.close()

    async def test_valid_mcp_argument_error_allows_truthful_finish_but_keeps_failed_operation(self):
        actual_popen = subprocess.Popen
        def controlled_process(argv, **kwargs):
            return actual_popen([sys.executable,'-u','-c',SERVER], **kwargs)

        with TemporaryDirectory() as directory:
            with patch('evals.paired.memory_quality_diagnostic.subprocess.Popen',side_effect=controlled_process):
                binding = MCP({'product_binary':sys.executable,'launch_env':{}},Path(directory))
            try:
                host = SkillHost('memory-mcp',{}, {'mcp__memory__explain':lambda args:binding.tool('explain',args)})
                actions = iter([
                    {'action':'call','name':'mcp__memory__explain','arguments':{'context_items':'[]'}},
                    {'action':'finish','answer':'No source was verified: source_episode is required.'}])
                async def request(messages,deadline):
                    return {'choices':[{'message':{'content':json.dumps(next(actions))},'finish_reason':'stop'}]}

                result = await play([],host,request)

                self.assertEqual(result['terminal'],'answered')
                self.assertTrue(host.trace[0]['response']['denied'])
                self.assertEqual(host.trace[0]['response']['mcp_response'],{
                    'jsonrpc':'2.0','id':3,'error':{'code':-32602,'message':'source_episode required'}})
                self.assertIn('source_episode required',result['turns'][1]['request_messages'][-1]['content'])
                self.assertFalse(grade_observed({'skill':'memory-mcp','id':2},result,host.trace)['operation_and_exact_arguments'])
            finally:
                binding.close()
