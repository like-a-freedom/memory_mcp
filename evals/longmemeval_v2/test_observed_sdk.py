"""Optional integration with upstream's SDK; portable stdlib controls run separately."""
import asyncio
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
import os
from pathlib import Path
from tempfile import TemporaryDirectory
import threading
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import patch

from evals.longmemeval_v2.inference_attempts import ACTIVE_CALL, AttemptRecorder, observed_sdk_clients


@contextmanager
def sdk_endpoint():
    release, calls = threading.Event(), []
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass
        def do_POST(self):
            body = self.rfile.read(int(self.headers['Content-Length']))
            calls.append(json.loads(body))
            if len(calls) == 1:
                release.wait(2)
                return
            release.set()
            result = json.dumps({'id':'local','model':'test','object':'chat.completion',
                'choices':[{'index':0,'message':{'role':'assistant','content':'ok'},'finish_reason':'stop'}]}).encode()
            self.send_response(200)
            self.send_header('Content-Type','application/json')
            self.send_header('Content-Length',str(len(result)))
            self.end_headers()
            self.wfile.write(result)
    server = ThreadingHTTPServer(('127.0.0.1',0),Handler)
    thread = threading.Thread(target=lambda: server.serve_forever(poll_interval=0.01))
    thread.start()
    try:
        yield f'http://127.0.0.1:{server.server_port}/v1', calls
    finally:
        release.set()
        server.shutdown()
        server.server_close()
        thread.join(2)
        if thread.is_alive():
            raise AssertionError('server still running')


@unittest.skipUnless(importlib.util.find_spec('openai'), 'requires optional upstream SDK environment')
class ObservedSdkTests(unittest.TestCase):
    def run_role(self, role):
        from openai import AsyncOpenAI, OpenAI
        with TemporaryDirectory() as directory, sdk_endpoint() as (url,calls):
            recorder = AttemptRecorder(Path(directory),max_retries=1,timeout_seconds=0.1)
            evaluation, qa = ModuleType('evaluation'), ModuleType('evaluation.qa_eval_metrics')
            evaluation.qa_eval_metrics = qa
            qa._create_openai_client = lambda: OpenAI(base_url=url,api_key='EMPTY',max_retries=10)
            harness = SimpleNamespace(create_async_client=lambda: AsyncOpenAI(base_url=url,api_key='EMPTY',max_retries=10))
            identity = ACTIVE_CALL.set(('a',role))
            try:
                with patch.dict('sys.modules',{'evaluation':evaluation,'evaluation.qa_eval_metrics':qa}), observed_sdk_clients(harness,recorder):
                    if role == 'reader':
                        async def complete():
                            async with harness.create_async_client() as client:
                                return await client.chat.completions.create(model='test',messages=[])
                        response = asyncio.run(complete())
                    else:
                        response = qa._create_openai_client().chat.completions.create(model='test',messages=[])
            finally:
                ACTIVE_CALL.reset(identity)
            rows = [json.loads(line) for line in (Path(directory)/'inference-attempts.jsonl').read_text().splitlines()]
            self.assertEqual(len(calls),2)
            self.assertEqual([r['outcome'] for r in rows],['timeout','success'])
            self.assertEqual(response.choices[0].message.content,'ok')
            self.assertEqual({r['logical_id'] for r in rows},{f'{role}:a:1'})

    def test_reader_sdk_retries_disabled_and_deadline_observed(self):
        self.run_role('reader')

    def test_synchronous_judge_uses_bounded_async_transport(self):
        self.run_role('judge')

    def routing_observation(self, role, redirect):
        from openai import AsyncOpenAI,OpenAI
        calls = []
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_POST(self):
                self.rfile.read(int(self.headers['Content-Length']));calls.append(self.server.kind)
                if self.server.kind=='origin' and redirect:
                    self.send_response(307);self.send_header('Location',f'http://127.0.0.1:{trap.server_port}/v1/chat/completions');self.send_header('Content-Length','0');self.end_headers();return
                body=json.dumps({'id':'routing','object':'chat.completion','model':'test','choices':[{'index':0,'message':{'role':'assistant','content':'ok'},'finish_reason':'stop'}]}).encode()
                self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
        origin=ThreadingHTTPServer(('127.0.0.1',0),Handler);origin.kind='origin'
        trap=ThreadingHTTPServer(('127.0.0.1',0),Handler);trap.kind='trap'
        threads=[threading.Thread(target=s.serve_forever,daemon=True) for s in (origin,trap)]
        for t in threads:t.start()
        try:
            env={k:v for k,v in os.environ.items() if k.lower() not in ('http_proxy','https_proxy','all_proxy','no_proxy')}
            if not redirect:env.update(HTTP_PROXY=f'http://127.0.0.1:{trap.server_port}',HTTPS_PROXY=f'http://127.0.0.1:{trap.server_port}',NO_PROXY='')
            with TemporaryDirectory() as root,patch.dict(os.environ,env,clear=True):
                recorder=AttemptRecorder(Path(root),max_retries=0,timeout_seconds=2)
                url=f'http://127.0.0.1:{origin.server_port}/v1'
                evaluation,qa=ModuleType('evaluation'),ModuleType('evaluation.qa_eval_metrics');evaluation.qa_eval_metrics=qa
                qa._create_openai_client=lambda:OpenAI(base_url=url,api_key='EMPTY',max_retries=10)
                harness=SimpleNamespace(create_async_client=lambda:AsyncOpenAI(base_url=url,api_key='EMPTY',max_retries=10))
                identity=ACTIVE_CALL.set(('routing',role))
                error=None
                try:
                    with patch.dict('sys.modules',{'evaluation':evaluation,'evaluation.qa_eval_metrics':qa}),observed_sdk_clients(harness,recorder):
                        if role=='reader':
                            async def complete():
                                async with harness.create_async_client() as client:return await client.chat.completions.create(model='test',messages=[])
                            asyncio.run(complete())
                        else:qa._create_openai_client().chat.completions.create(model='test',messages=[])
                except Exception as exc:error=type(exc).__name__
                finally:ACTIVE_CALL.reset(identity)
                rows=[json.loads(line) for line in (Path(root)/'inference-attempts.jsonl').read_text().splitlines()]
                return calls,rows,error
        finally:
            for server in (origin,trap):server.shutdown();server.server_close()
            for t in threads:t.join(timeout=2)

    def test_reader_sdk_refuses_redirected_evidence(self):
        calls,rows,error=self.routing_observation('reader',True)
        self.assertEqual(calls,['origin'])
        self.assertEqual(rows[0]['http_status'],307)
        self.assertIsNotNone(error)

    def test_judge_sdk_refuses_redirected_evidence(self):
        calls,rows,error=self.routing_observation('judge',True)
        self.assertEqual(calls,['origin'])
        self.assertEqual(rows[0]['http_status'],307)
        self.assertIsNotNone(error)

    def test_reader_sdk_bypasses_environment_proxy(self):
        calls,rows,error=self.routing_observation('reader',False)
        self.assertEqual(calls,['origin'])
        self.assertEqual(rows[0]['outcome'],'success')
        self.assertIsNone(error)

    def test_judge_sdk_bypasses_environment_proxy(self):
        calls,rows,error=self.routing_observation('judge',False)
        self.assertEqual(calls,['origin'])
        self.assertEqual(rows[0]['outcome'],'success')
        self.assertIsNone(error)
