"""Real loopback fault controls; no model or vendor SDK required."""
import asyncio
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import socket
import subprocess
import sys
from tempfile import TemporaryDirectory
import threading
import unittest
from urllib.request import urlopen
from urllib.error import HTTPError

from evals.longmemeval_v2.inference_attempts import AttemptRecorder


@contextmanager
def endpoint(first):
    release = threading.Event()
    calls = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            calls.append(self.path)
            if first == 'malformed':
                self.send_response(200);self.end_headers();self.wfile.write(b'{"choices":[null]}');return
            if len(calls) == 1:
                if first == 'rate_limit':
                    self.send_response(429)
                    self.end_headers()
                    return
                if first == 'timeout':
                    release.wait(2)
                    return
                self.connection.shutdown(socket.SHUT_RDWR)
                self.connection.close()
                return
            release.set()
            if first == 'disconnect':
                self.connection.shutdown(socket.SHUT_RDWR)
                self.connection.close()
                return
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}')

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    worker = threading.Thread(target=lambda: server.serve_forever(poll_interval=0.01))
    worker.start()
    try:
        yield f'http://127.0.0.1:{server.server_port}', calls
    finally:
        release.set()
        server.shutdown()
        server.server_close()
        worker.join(2)
        if worker.is_alive():
            raise AssertionError('loopback cleanup exceeded its bound')


class AttemptTests(unittest.TestCase):
    def test_selector_http_fault_is_not_a_connection_error(self):
        with TemporaryDirectory() as root, endpoint('rate_limit') as (url, calls):
            recorder = AttemptRecorder(Path(root), max_retries=1, timeout_seconds=1)
            def request():
                try:
                    with urlopen(url, timeout=1) as response:
                        return json.load(response)
                except HTTPError as error:
                    error.close()
                    raise
            recorder.call('independent-query','selector',request)
            rows = [json.loads(line) for line in (Path(root)/'inference-attempts.jsonl').read_text().splitlines()]
            self.assertEqual(len(calls),2)
            self.assertEqual([r['outcome'] for r in rows],['http_error','success'])
            self.assertEqual(rows[0]['http_status'],429)
            self.assertEqual({r['role'] for r in rows},{'selector'})

    def test_timeout_then_success_retains_two_attempts_and_one_terminal_case(self):
        with TemporaryDirectory() as root, endpoint('timeout') as (url, calls):
            recorder = AttemptRecorder(Path(root), max_retries=1, timeout_seconds=0.05)

            def request():
                with urlopen(url, timeout=recorder.timeout_seconds) as response:
                    return json.load(response)

            result = asyncio.run(recorder.call_async('case', 'reader', lambda: asyncio.to_thread(request)))
            self.assertEqual(result['choices'][0]['finish_reason'], 'stop')
            rows = [json.loads(line) for line in (Path(root) / 'inference-attempts.jsonl').read_text().splitlines()]
            self.assertEqual(len(calls), 2)
            self.assertEqual([row['outcome'] for row in rows], ['timeout', 'success'])
            self.assertEqual([row['attempt'] for row in rows], [1, 2])
            self.assertEqual({row['logical_id'] for row in rows}, {'reader:case:1'})
            self.assertEqual([row['request_id'] for row in rows], ['reader:case:1:1', 'reader:case:1:2'])
            self.assertEqual([row['terminal'] for row in rows], [False, True])
            self.assertIsNone(rows[-1]['prefill_seconds'])
            self.assertIsNone(rows[-1]['usage'])
            self.assertEqual(rows[-1]['finish_reason'], 'stop')
            self.assertEqual(rows[0]['temperature_state'], 'first_observed_request')
            self.assertEqual(rows[1]['temperature_state'], 'subsequent_request')
            self.assertLess(sum(row['elapsed_seconds'] for row in rows), 1)

    def test_disconnect_exhausts_finite_budget_without_turning_failure_into_empty_answer(self):
        with TemporaryDirectory() as root, endpoint('disconnect') as (url, calls):
            recorder = AttemptRecorder(Path(root), max_retries=1, timeout_seconds=0.2)

            def request():
                with urlopen(url, timeout=recorder.timeout_seconds) as response:
                    return json.load(response)

            with self.assertRaises(ConnectionError):
                recorder.call('case', 'judge', request)
            rows = [json.loads(line) for line in (Path(root) / 'inference-attempts.jsonl').read_text().splitlines()]
            self.assertEqual(len(calls), 2)
            self.assertEqual([row['outcome'] for row in rows], ['transport_error', 'transport_error'])
            self.assertEqual([row['terminal'] for row in rows], [False, True])
            self.assertEqual({row['role'] for row in rows}, {'judge'})

    def test_semantically_invalid_response_is_not_retried(self):
        with TemporaryDirectory() as root:
            recorder = AttemptRecorder(Path(root), max_retries=1, timeout_seconds=1)
            with self.assertRaises(ValueError):
                recorder.call('case', 'reader', lambda: json.loads('broken'))
            rows = (Path(root) / 'inference-attempts.jsonl').read_text().splitlines()
            self.assertEqual(len(rows), 1)
            self.assertEqual(json.loads(rows[0])['outcome'], 'invalid_response')

    def test_malformed_envelope_retains_the_actual_invalid_attempt(self):
        with TemporaryDirectory() as root, endpoint('malformed') as (url,calls):
            recorder = AttemptRecorder(Path(root),max_retries=1,timeout_seconds=1)
            def request():
                with urlopen(url,timeout=1) as response:return json.load(response)
            with self.assertRaises(Exception) as raised:recorder.call('malformed','reader',request)
            self.assertIsInstance(raised.exception,ValueError)
            rows = [json.loads(line) for line in (Path(root)/'inference-attempts.jsonl').read_text().splitlines()]
            self.assertEqual(len(calls),1)
            self.assertEqual(rows[0]['outcome'],'invalid_response')
            self.assertTrue(rows[0]['terminal'])
            self.assertEqual(rows[0]['response_raw'],{'choices':[None]})

    def test_terminated_issued_request_retains_its_pending_identity(self):
        issued,release = threading.Event(),threading.Event()
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*_args):pass
            def do_GET(self):
                issued.set();release.wait(3)
        server = ThreadingHTTPServer(('127.0.0.1',0),Handler)
        thread = threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            with TemporaryDirectory() as root:
                script = "from pathlib import Path;import sys,json;from urllib.request import urlopen;from evals.longmemeval_v2.inference_attempts import AttemptRecorder;r=AttemptRecorder(Path(sys.argv[1]),max_retries=0,timeout_seconds=5);r.call('abandoned','reader',lambda:json.load(urlopen(sys.argv[2],timeout=5)))"
                process = subprocess.Popen([sys.executable,'-c',script,root,f'http://127.0.0.1:{server.server_port}'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
                try:
                    self.assertTrue(issued.wait(2),'actual request never reached controlled peer')
                    process.terminate();process.communicate(timeout=2)
                    self.assertTrue((Path(root)/'inference-events.jsonl').exists(),'issued request lost its pending identity')
                    events = [json.loads(line) for line in (Path(root)/'inference-events.jsonl').read_text().splitlines()]
                    self.assertEqual(len(events),1)
                    self.assertEqual(events[0]['event'],'attempt_started')
                    self.assertEqual(events[0]['request_id'],'reader:abandoned:1:1')
                    self.assertFalse(events[0]['terminal'])
                finally:
                    if process.poll() is None:process.kill();process.communicate(timeout=2)
        finally:
            release.set();server.shutdown();server.server_close();thread.join(timeout=2)
