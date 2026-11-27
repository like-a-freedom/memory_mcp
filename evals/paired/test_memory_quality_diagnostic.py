"""Public CLI grading controls for the bounded local diagnostic."""
import json
from contextlib import contextmanager
import os
from unittest.mock import patch
import hashlib
import http.server
import pathlib
import subprocess
import sys
import tempfile
import threading
import unittest

SCRIPT = pathlib.Path(__file__).with_name("memory_quality_diagnostic.py").resolve()

@contextmanager
def one_fixed_capture(handler, *, environment=None):
    """One public CLI action; controlled MCP adapter, real HTTP fault boundary."""
    server = http.server.ThreadingHTTPServer(('127.0.0.1',0),handler)
    worker = threading.Thread(target=server.serve_forever,daemon=True);worker.start()
    try:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root/'serve').write_text("import sys,json\nfor line in sys.stdin:\n q=json.loads(line)\n if 'id' not in q:continue\n result={}\n if q['method']=='tools/call':result={'structuredContent':{'status':'success','result':'episode:source' if q['params']['name']=='ingest' else {'facts':[{'fact_id':'fact:one'}]}}}\n print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':result}),flush=True)\n")
            case = {'id':'transport','question':'Color?','query':'color','events':[{'id':'source','content':'Blue.','t_ref':'2026-11-01T10:00:00Z'}],'answer':'Blue','source':'source','abstain':False}
            plan = {'script_sha256':hashlib.sha256(SCRIPT.read_bytes()).hexdigest(),'product_binary':sys.executable,'product_binary_sha256':hashlib.sha256(pathlib.Path(sys.executable).read_bytes()).hexdigest(),'model_budget_seconds':5,'per_call_seconds':1,'launch_env':{},'cases':[case],'arms':['fixed_sufficient_reply'],'endpoint':f'http://127.0.0.1:{server.server_port}/v1/chat/completions','model':'controlled','sampling':{},'reader_system':'Return JSON.'}
            (root/'frozen-plan.json').write_text(json.dumps(plan))
            completed = subprocess.run([sys.executable,str(SCRIPT),'capture','--output',str(root)],cwd=root,env=environment,capture_output=True,text=True,timeout=8)
            yield root,completed
    finally:
        server.shutdown();server.server_close();worker.join(timeout=2)

class DiagnosticGrading(unittest.TestCase):
    def score(self, expected, answer):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / "case.json").write_text(json.dumps(expected))
            (root / "answer.json").write_text(json.dumps(answer))
            result = subprocess.run([sys.executable, str(SCRIPT), "score", "--case", str(root / "case.json"), "--answer", str(root / "answer.json")], capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)

    def test_numeric_prefix_is_not_exact_value(self):
        result = self.score({"answer":"47", "source_ids":["episode:source"], "abstain":False}, {"answer":"470", "source_ids":["episode:source"], "abstain":False})
        self.assertFalse(result["value_correct"])
        self.assertFalse(result["passed"])

    def test_correct_value_with_wrong_source_fails(self):
        result = self.score({"answer":"47", "source_ids":["episode:source"], "abstain":False}, {"answer":"47", "source_ids":["episode:other"], "abstain":False})
        self.assertFalse(result["passed"])
        self.assertFalse(result["sources_correct"])

    def test_raw_ids_cannot_hide_an_unknown_second_citation_channel(self):
        expected = {"answer":"47", "source_ids":["episode:source"], "abstain":False}
        actual = {**expected, "source_refs":["unknown"]}
        self.assertFalse(self.score(expected, actual)["passed"])

    def test_raw_citation_cannot_hide_a_conflicting_unregistered_channel(self):
        expected = {'answer':'47','source_ids':['episode:source'],'abstain':False}
        self.assertFalse(self.score(expected,{**expected,'citation':['episode:wrong']})['passed'])

    def test_handle_citation_cannot_hide_a_conflicting_unregistered_channel(self):
        expected = {'answer':'47','source_ids':['episode:source'],'abstain':False,'citation_mode':'handles','citation_map':{'s1':'episode:source'}}
        self.assertFalse(self.score(expected,{'answer':'47','source_refs':['s1'],'abstain':False,'citation':['s2']})['passed'])

    def test_selected_public_source_handle_resolves_to_its_exact_id(self):
        expected = {"answer":"47", "source_ids":["episode:0123456789abcdef01234567"], "abstain":False, "citation_mode":"handles", "citation_map":{"s1":"episode:other","s2":"episode:0123456789abcdef01234567"}}
        actual = {"answer":"47", "source_refs":["s2"], "abstain":False}
        self.assertTrue(self.score(expected,actual)['passed'])

    def test_wrong_returned_source_handle_is_not_repaired(self):
        expected = {"answer":"47", "source_ids":["episode:correct"], "abstain":False, "citation_mode":"handles", "citation_map":{"s1":"episode:other","s2":"episode:correct"}}
        actual = {"answer":"47", "source_refs":["s1"], "abstain":False}
        self.assertFalse(self.score(expected,actual)['sources_correct'])

    def test_absent_source_handle_is_not_invented(self):
        expected = {"answer":"47", "source_ids":["episode:correct"], "abstain":False, "citation_mode":"handles", "citation_map":{"s1":"episode:correct"}}
        actual = {"answer":"47", "source_refs":["s2"], "abstain":False}
        self.assertFalse(self.score(expected,actual)['passed'])

    def test_duplicate_source_handle_is_not_a_single_citation(self):
        expected = {"answer":"47", "source_ids":["episode:correct"], "abstain":False, "citation_mode":"handles", "citation_map":{"s1":"episode:correct"}}
        actual = {"answer":"47", "source_refs":["s1","s1"], "abstain":False}
        self.assertFalse(self.score(expected,actual)['passed'])

    def test_missing_answer_is_not_justified_abstention(self):
        result = self.score({"answer":None, "source_ids":["episode:pending"], "abstain":True}, {"source_ids":["episode:pending"], "abstain":True})
        self.assertFalse(result["passed"])

    def test_exact_answer_and_evidence_are_accepted(self):
        expected = {"answer":"47", "source_ids":["episode:source"], "abstain":False}
        self.assertTrue(self.score(expected, expected)["passed"])

    def test_supported_false_premise_abstention_is_accepted(self):
        expected = {"answer":None, "source_ids":["episode:pending"], "abstain":True}
        self.assertTrue(self.score(expected, expected)["passed"])

    def test_a_complete_reordered_set_preserves_the_closed_answer(self):
        expected = {"answer":["Birch", "Cedar", "Elm"], "answer_mode":"set", "source_ids":["episode:menu"], "abstain":False}
        actual = {"answer":["Elm", "Birch", "Cedar"], "source_ids":["episode:menu"], "abstain":False}
        self.assertTrue(self.score(expected, actual)["passed"])

    def test_a_correct_value_with_a_fabricated_quote_fails(self):
        expected = {"answer":"9", "quote":"The toolbar has nine buttons.", "source_ids":["episode:toolbar"], "abstain":False}
        actual = {"answer":"9", "quote":"The toolbar has ninety buttons.", "source_ids":["episode:toolbar"], "abstain":False}
        self.assertFalse(self.score(expected, actual)["passed"])

    def test_exact_delivered_proposition_does_not_require_a_record_label(self):
        expected = {"answer":"9", "quote":"There are nine buttons.", "quote_mode":"source_span", "quote_source":"Fact: There are nine buttons.", "quote_evidence":["There are nine buttons."], "source_ids":["episode:toolbar"], "abstain":False}
        actual = {"answer":"9", "quote":"There are nine buttons.", "source_ids":["episode:toolbar"], "abstain":False}
        self.assertTrue(self.score(expected, actual)["passed"])

    def test_verbatim_source_record_can_include_its_label(self):
        expected = {"answer":"9", "quote":"There are nine buttons.", "quote_mode":"source_span", "quote_source":"Fact: There are nine buttons.", "quote_evidence":["Fact: There are nine buttons."], "source_ids":["episode:toolbar"], "abstain":False}
        actual = {"answer":"9", "quote":"Fact: There are nine buttons.", "source_ids":["episode:toolbar"], "abstain":False}
        self.assertTrue(self.score(expected, actual)["passed"])

    def test_source_quote_cannot_be_reconstructed_from_a_truncated_read(self):
        expected = {"answer":"9", "quote":"There are nine buttons.", "quote_mode":"source_span", "quote_source":"Fact: There are nine buttons.", "quote_evidence":["There are nine"], "source_ids":["episode:toolbar"], "abstain":False}
        actual = {"answer":"9", "quote":"There are nine buttons.", "source_ids":["episode:toolbar"], "abstain":False}
        self.assertFalse(self.score(expected, actual)["passed"])

    def test_partial_source_quote_cannot_support_the_full_registered_proposition(self):
        expected = {"answer":"9", "quote":"There are nine buttons.", "quote_mode":"source_span", "quote_source":"Fact: There are nine buttons.", "quote_evidence":["Fact: There are nine buttons."], "source_ids":["episode:toolbar"], "abstain":False}
        actual = {"answer":"9", "quote":"nine", "source_ids":["episode:toolbar"], "abstain":False}
        self.assertFalse(self.score(expected, actual)["passed"])

    def test_capture_does_not_borrow_quote_from_an_uncited_returned_source(self):
        answer = {"answer":"9", "quote":"There are nine buttons.", "source_refs":["s1"], "abstain":False}
        class Model(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers['Content-Length']))
                body = json.dumps({"choices":[{"message":{"content":json.dumps(answer)}}]}).encode()
                self.send_response(200); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
            def log_message(self, *args):
                pass
        server = http.server.HTTPServer(('127.0.0.1',0),Model)
        worker = threading.Thread(target=server.serve_forever,daemon=True); worker.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                binary = pathlib.Path(sys.executable)
                (root / 'serve').write_text('''import sys,json
for line in sys.stdin:
 q=json.loads(line)
 if 'id' not in q: continue
 result={}
 if q['method']=='tools/call':
  tool=q['params']['name']
  value='episode:expected' if tool=='ingest' else {'facts':[{'fact_id':'fact:one'}]} if tool=='extract' else [{'source_episode':'episode:expected','content':'There are nine'},{'source_episode':'episode:other','content':'There are nine buttons.'}]
  result={'structuredContent':{'result':value}}
 print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':result}),flush=True)
''')
                case = {"id":"cited-evidence", "question":"How many?", "query":"buttons", "events":[{"id":"source","content":"Fact: There are nine buttons.","t_ref":"2026-11-01T10:00:00Z"}],"answer":"9","source":"source","abstain":False,"quote":"There are nine buttons.","quote_mode":"source_span","citation_mode":"handles"}
                plan = {"script_sha256":hashlib.sha256(SCRIPT.read_bytes()).hexdigest(), "product_binary":str(binary), "product_binary_sha256":hashlib.sha256(binary.read_bytes()).hexdigest(), "model_budget_seconds":10, "per_call_seconds":2, "launch_env":{}, "cases":[case], "arms":["registered_query"], "endpoint":f"http://127.0.0.1:{server.server_port}/v1/chat/completions", "model":"controlled-adapter", "sampling":{}, "reader_system":"Return JSON."}
                (root / 'frozen-plan.json').write_text(json.dumps(plan))
                result = subprocess.run([sys.executable,str(SCRIPT),'capture','--output',str(root)],cwd=root,capture_output=True,text=True,timeout=10)
                self.assertEqual(result.returncode,0,result.stderr)
                row = json.loads((root / 'result.json').read_text())['captures'][0]['rows'][0]
                self.assertEqual(row['status'],'completed',row)
                self.assertEqual(row['expected']['citation_map'],{'s1':'episode:expected','s2':'episode:other'})
                raw = json.loads((root / 'capture-01/cited-evidence-registered_query.reader.json').read_text())
                self.assertEqual(json.loads(raw['request']['messages'][1]['content'])['citation_map'],row['expected']['citation_map'])
                self.assertEqual(row['actual'],answer)
                self.assertEqual(row['scores']['resolved_source_ids'],['episode:expected'])
                self.assertTrue(row['scores']['sources_correct'])
                self.assertFalse(row['scores']['quote_correct'])
                self.assertFalse(row['scores']['passed'])
        finally:
            server.shutdown(); server.server_close(); worker.join(timeout=2)

    def test_an_extra_element_is_not_a_complete_supported_set(self):
        expected = {"answer":["Birch", "Cedar"], "answer_mode":"set", "source_ids":["episode:menu"], "abstain":False}
        actual = {"answer":["Birch", "Cedar", "Elm"], "source_ids":["episode:menu"], "abstain":False}
        self.assertFalse(self.score(expected, actual)["value_correct"])

    def test_fixed_reply_preserves_registered_distractors_and_source_order(self):
        class Model(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers['Content-Length']))
                answer = {'answer':'Blue','source_ids':['episode:gold'],'abstain':False}
                body = json.dumps({'choices':[{'message':{'content':json.dumps(answer)},'finish_reason':'stop'}]}).encode()
                self.send_response(200); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
            def log_message(self, *args): pass
        server = http.server.HTTPServer(('127.0.0.1',0),Model)
        worker = threading.Thread(target=server.serve_forever,daemon=True); worker.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                (root/'serve').write_text('''import sys,json
for line in sys.stdin:
 q=json.loads(line)
 if 'id' not in q:continue
 result={}
 if q['method']=='tools/call':
  tool=q['params']['name'];args=q['params']['arguments']
  result={'structuredContent':{'result':'episode:'+args['source_id'] if tool=='ingest' else {'facts':[{'fact_id':'fact:one'}]}}}
 print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':result}),flush=True)
''')
                events = [{'id':key,'content':text,'t_ref':'2026-11-01T10:00:00Z'} for key,text in [('other','Red.'),('gold','Blue.'),('third','Green.')]]
                case = {'id':'source-order','question':'Color?', 'query':'color', 'events':events, 'answer':'Blue','source':'gold','abstain':False,'fixed_evidence_sources':['third','gold','other']}
                plan = {'script_sha256':hashlib.sha256(SCRIPT.read_bytes()).hexdigest(),'product_binary':sys.executable,'product_binary_sha256':hashlib.sha256(pathlib.Path(sys.executable).read_bytes()).hexdigest(),'model_budget_seconds':10,'per_call_seconds':2,'launch_env':{},'cases':[case],'arms':['fixed_sufficient_reply'],'endpoint':f'http://127.0.0.1:{server.server_port}/v1/chat/completions','model':'controlled','sampling':{},'reader_system':'Return JSON.'}
                (root/'frozen-plan.json').write_text(json.dumps(plan))
                result = subprocess.run([sys.executable,str(SCRIPT),'capture','--output',str(root)],cwd=root,capture_output=True,text=True,timeout=15)
                self.assertEqual(result.returncode,0,result.stderr)
                evidence = json.loads((root/'capture-01/source-order-fixed_sufficient_reply.evidence.json').read_text())
                self.assertEqual([item['source_episode_ids'][0] for item in evidence['context_items']],['episode:third','episode:gold','episode:other'])
                attempts = [json.loads(line) for line in (root/'capture-01/inference-attempts.jsonl').read_text().splitlines()]
                self.assertEqual(len(attempts),1)
                self.assertTrue(attempts[0]['terminal'])
        finally:
            server.shutdown();server.server_close();worker.join(timeout=2)

    def test_successful_episode_only_can_supply_explicit_fixed_evidence(self):
        class Model(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers['Content-Length']))
                answer = {'answer':'Blue','source_ids':['episode:gold'],'abstain':False}
                body = json.dumps({'choices':[{'message':{'content':json.dumps(answer)},'finish_reason':'stop'}]}).encode()
                self.send_response(200); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
            def log_message(self, *args): pass
        server = http.server.HTTPServer(('127.0.0.1',0),Model)
        worker = threading.Thread(target=server.serve_forever,daemon=True); worker.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                (root/'serve').write_text('''import sys,json
for line in sys.stdin:
 q=json.loads(line)
 if 'id' not in q:continue
 result={}
 if q['method']=='tools/call':
  tool=q['params']['name'];args=q['params']['arguments']
  result={'structuredContent':{'status':'success','result':'episode:'+args['source_id'] if tool=='ingest' else {'facts':[]}}}
 print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':result}),flush=True)
''')
                events = [{'id':key,'content':text,'t_ref':'2026-11-01T10:00:00Z'} for key,text in [('other','Red.'),('gold','Blue.'),('third','Green.')]]
                case = {'id':'source-order','question':'Color?', 'query':'color', 'events':events, 'answer':'Blue','source':'gold','abstain':False,'fixed_evidence_sources':['third','gold','other']}
                plan = {'script_sha256':hashlib.sha256(SCRIPT.read_bytes()).hexdigest(),'product_binary':sys.executable,'product_binary_sha256':hashlib.sha256(pathlib.Path(sys.executable).read_bytes()).hexdigest(),'formation_policy':'stored_episode','model_budget_seconds':10,'per_call_seconds':2,'launch_env':{},'cases':[case],'arms':['fixed_sufficient_reply'],'endpoint':f'http://127.0.0.1:{server.server_port}/v1/chat/completions','model':'controlled','sampling':{},'reader_system':'Return JSON.'}
                (root/'frozen-plan.json').write_text(json.dumps(plan))
                result = subprocess.run([sys.executable,str(SCRIPT),'capture','--output',str(root)],cwd=root,capture_output=True,text=True,timeout=15)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual(json.loads((root/'capture-01/rows.json').read_text())[0]['status'],'completed')
                evidence = json.loads((root/'capture-01/source-order-fixed_sufficient_reply.evidence.json').read_text())
                self.assertEqual([item['source_episode_ids'][0] for item in evidence['context_items']],['episode:third','episode:gold','episode:other'])
                attempts = [json.loads(line) for line in (root/'capture-01/inference-attempts.jsonl').read_text().splitlines()]
                self.assertEqual(len(attempts),1)
                self.assertTrue(attempts[0]['terminal'])
        finally:
            server.shutdown();server.server_close();worker.join(timeout=2)

    def test_partial_extraction_cannot_supply_verified_fixed_evidence(self):
        class Model(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers['Content-Length']))
                answer = {'answer':'Blue','source_ids':['episode:gold'],'abstain':False}
                body = json.dumps({'choices':[{'message':{'content':json.dumps(answer)},'finish_reason':'stop'}]}).encode()
                self.send_response(200); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
            def log_message(self, *args): pass
        server = http.server.HTTPServer(('127.0.0.1',0),Model)
        worker = threading.Thread(target=server.serve_forever,daemon=True); worker.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                (root/'serve').write_text('''import sys,json
for line in sys.stdin:
 q=json.loads(line)
 if 'id' not in q:continue
 result={}
 if q['method']=='tools/call':
  tool=q['params']['name'];args=q['params']['arguments']
  result={'structuredContent':{'status':'partial','result':'episode:'+args['source_id'] if tool=='ingest' else {'facts':[]}}}
 print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':result}),flush=True)
''')
                events = [{'id':key,'content':text,'t_ref':'2026-11-01T10:00:00Z'} for key,text in [('other','Red.'),('gold','Blue.'),('third','Green.')]]
                case = {'id':'source-order','question':'Color?', 'query':'color', 'events':events, 'answer':'Blue','source':'gold','abstain':False,'fixed_evidence_sources':['third','gold','other']}
                plan = {'script_sha256':hashlib.sha256(SCRIPT.read_bytes()).hexdigest(),'product_binary':sys.executable,'product_binary_sha256':hashlib.sha256(pathlib.Path(sys.executable).read_bytes()).hexdigest(),'formation_policy':'stored_episode','model_budget_seconds':10,'per_call_seconds':2,'launch_env':{},'cases':[case],'arms':['fixed_sufficient_reply'],'endpoint':f'http://127.0.0.1:{server.server_port}/v1/chat/completions','model':'controlled','sampling':{},'reader_system':'Return JSON.'}
                (root/'frozen-plan.json').write_text(json.dumps(plan))
                result = subprocess.run([sys.executable,str(SCRIPT),'capture','--output',str(root)],cwd=root,capture_output=True,text=True,timeout=15)
                self.assertEqual(result.returncode,0,result.stderr)
                row = json.loads((root/'capture-01/rows.json').read_text())[0]
                self.assertEqual(row['status'],'invalid')
                self.assertIn('incomplete formation',row['error'])
                self.assertFalse((root/'capture-01/inference-attempts.jsonl').exists())
        finally:
            server.shutdown();server.server_close();worker.join(timeout=2)

    def test_trickling_response_cannot_extend_the_whole_call_deadline(self):
        stopped = threading.Event()
        class Model(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers['Content-Length']))
                body = json.dumps({'choices':[{'message':{'content':'{"answer":"Blue","source_ids":["episode:gold"],"abstain":false}'},'finish_reason':'stop'}]}).encode()
                self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers()
                try:
                    for byte in body:
                        if stopped.wait(.025):break
                        self.wfile.write(bytes([byte]));self.wfile.flush()
                except (BrokenPipeError,ConnectionResetError):pass
            def log_message(self, *args):pass
        server = http.server.ThreadingHTTPServer(('127.0.0.1',0),Model)
        worker = threading.Thread(target=server.serve_forever,daemon=True);worker.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                (root/'serve').write_text("import sys,json\nfor line in sys.stdin:\n q=json.loads(line)\n if 'id' not in q:continue\n result={}\n if q['method']=='tools/call':result={'structuredContent':{'result':'episode:gold' if q['params']['name']=='ingest' else {'facts':[{'fact_id':'fact:one'}]}}}\n print(json.dumps({'jsonrpc':'2.0','id':q['id'],'result':result}),flush=True)\n")
                case = {'id':'trickle','question':'Color?','query':'color','events':[{'id':'gold','content':'Blue.','t_ref':'2026-11-01T10:00:00Z'}],'answer':'Blue','source':'gold','abstain':False}
                plan = {'script_sha256':hashlib.sha256(SCRIPT.read_bytes()).hexdigest(),'product_binary':sys.executable,'product_binary_sha256':hashlib.sha256(pathlib.Path(sys.executable).read_bytes()).hexdigest(),'model_budget_seconds':5,'per_call_seconds':.2,'launch_env':{},'cases':[case],'arms':['fixed_sufficient_reply'],'endpoint':f'http://127.0.0.1:{server.server_port}/v1/chat/completions','model':'controlled','sampling':{},'reader_system':'Return JSON.'}
                (root/'frozen-plan.json').write_text(json.dumps(plan))
                result = subprocess.run([sys.executable,str(SCRIPT),'capture','--output',str(root)],cwd=root,capture_output=True,text=True,timeout=3)
                self.assertEqual(result.returncode,0,result.stderr)
                rows = json.loads((root/'capture-01/rows.json').read_text())
                self.assertEqual(len(rows),1)
                self.assertEqual(rows[0]['status'],'invalid')
                attempts = [json.loads(line) for line in (root/'capture-01/inference-attempts.jsonl').read_text().splitlines()]
                self.assertEqual(len(attempts),1)
                self.assertEqual(attempts[0]['outcome'],'timeout')
                self.assertTrue(attempts[0]['terminal'])
        finally:
            stopped.set();server.shutdown();server.server_close();worker.join(timeout=2)

    def test_a_duplicate_cannot_replace_a_missing_set_element(self):
        expected = {"answer":["Birch", "Cedar"], "answer_mode":"set", "source_ids":["episode:menu"], "abstain":False}
        actual = {"answer":["Birch", "Birch"], "source_ids":["episode:menu"], "abstain":False}
        self.assertFalse(self.score(expected, actual)["value_correct"])

    def test_freeze_uses_the_declared_closed_evidence_tasks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            case = {"id":"closed-list", "question":"List every station.", "query":"complete station list", "events":[{"id":"menu","content":"Stations: Birch, Cedar.","t_ref":"2026-11-01T10:00:00Z"}],"answer":["Birch","Cedar"],"answer_mode":"set","source":"menu","abstain":False,"quote":"Stations: Birch, Cedar."}
            config = root / "config.json"
            config.write_text(json.dumps({"model":"opaque-model-alias", "endpoint":"http://127.0.0.1:18767/v1/chat/completions", "launch_env":{}, "cases":[case], "reader_system":"Return the requested value, exact quote and source IDs."}))
            result = subprocess.run([sys.executable,str(SCRIPT),"freeze","--output",str(root / "frozen"),"--binary",sys.executable,"--config",str(config)],capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,0,result.stderr)
            plan = json.loads((root / "frozen/frozen-plan.json").read_text())
            self.assertEqual(plan["cases"],[case])
            self.assertEqual(plan["reader_system"],"Return the requested value, exact quote and source IDs.")

    def test_fixed_only_arm_is_registered_without_scheduling_extra_reads(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            config = root / 'config.json'
            config.write_text(json.dumps({'model':'opaque', 'endpoint':'http://127.0.0.1:18767/v1/chat/completions', 'launch_env':{}, 'arms':['fixed_sufficient_reply']}))
            result = subprocess.run([sys.executable,str(SCRIPT),'freeze','--output',str(root/'frozen'),'--binary',sys.executable,'--config',str(config)],capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(json.loads((root/'frozen/frozen-plan.json').read_text())['arms'],['fixed_sufficient_reply'])

    def test_freeze_rejects_an_unregistered_fixed_evidence_source(self):
        case = {'id':'unknown-evidence','question':'Color?', 'query':'color', 'events':[{'id':'source','content':'Blue.'}], 'answer':'Blue', 'source':'source', 'abstain':False, 'fixed_evidence_sources':['absent']}
        result = self.freeze_case(case)
        self.assertNotEqual(result.returncode,0)
        self.assertIn('fixed_evidence_sources',result.stderr)

    def freeze_case(self, case):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            config = root / "config.json"
            config.write_text(json.dumps({"model":"opaque-model-alias", "endpoint":"http://127.0.0.1:18767/v1/chat/completions", "launch_env":{}, "cases":[case]}))
            return subprocess.run([sys.executable,str(SCRIPT),"freeze","--output",str(root / "frozen"),"--binary",sys.executable,"--config",str(config)],capture_output=True,text=True,timeout=10)

    def test_freeze_rejects_quote_not_in_the_cited_source(self):
        case = {"id":"quote-origin", "question":"What color?", "query":"color", "events":[{"id":"cited","content":"Blue."},{"id":"other","content":"Red."}],"answer":"Red","source":"cited","abstain":False,"quote":"Red."}
        result = self.freeze_case(case)
        self.assertNotEqual(result.returncode,0)
        self.assertIn("quote must occur in the cited source",result.stderr)

    def test_freeze_rejects_non_unique_closed_set_oracle(self):
        case = {"id":"duplicate-label", "question":"List stations.", "query":"stations", "events":[{"id":"menu","content":"Birch."}],"answer":["Birch","Birch"],"answer_mode":"set","source":"menu","abstain":False}
        result = self.freeze_case(case)
        self.assertNotEqual(result.returncode,0)
        self.assertIn("set answers must contain unique strings",result.stderr)

    def test_freeze_rejects_unknown_answer_comparison(self):
        case = {"id":"comparison-typo", "question":"Color?", "query":"color", "events":[{"id":"source","content":"Blue."}],"answer":"Blue","answer_mode":"prefix","source":"source","abstain":False}
        result = self.freeze_case(case)
        self.assertNotEqual(result.returncode,0)
        self.assertIn("unsupported answer_mode",result.stderr)

    def test_startup_failure_preserves_all_scheduled_arms(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            plan = {"script_sha256":hashlib.sha256(SCRIPT.read_bytes()).hexdigest(), "product_binary":sys.executable, "product_binary_sha256":hashlib.sha256(pathlib.Path(sys.executable).read_bytes()).hexdigest(), "model_budget_seconds":1, "launch_env":{}, "cases":[{"id":"ordinary"},{"id":"correction"},{"id":"false-premise"}], "arms":["registered_query","selected_query","fixed_sufficient_reply"]}
            (root / "frozen-plan.json").write_text(json.dumps(plan))
            result = subprocess.run([sys.executable,str(SCRIPT),"capture","--output",str(root)],capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,0,result.stderr)
            report = json.loads((root / "result.json").read_text())
            self.assertEqual(len(report["captures"]),1)
            rows = report["captures"][0]["rows"]
            self.assertEqual(len(rows),9)
            self.assertTrue(all(row["status"] == "invalid" for row in rows))
            self.assertFalse(report["c2_eligible"])

    def test_freeze_accepts_opaque_model_id_without_local_model_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            config = root / "config.json"
            config.write_text(json.dumps({"model":"opaque-model-alias", "endpoint":"http://127.0.0.1:18767/v1/chat/completions", "launch_env":{}}))
            result = subprocess.run([sys.executable,str(SCRIPT),"freeze","--output",str(root / "frozen"),"--binary",sys.executable,"--config",str(config)],capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,0,result.stderr)
            plan = json.loads((root / "frozen/frozen-plan.json").read_text())
            self.assertEqual(plan["model"],"opaque-model-alias")
            self.assertIsNone(plan["model_config_sha256"])

    def test_existing_capture_result_cannot_be_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            config = root / "config.json"
            config.write_text(json.dumps({"model":"opaque-model-alias", "endpoint":"http://127.0.0.1:18767/v1/chat/completions", "launch_env":{}}))
            frozen = root / "frozen"
            result = subprocess.run([sys.executable,str(SCRIPT),"freeze","--output",str(frozen),"--binary",sys.executable,"--config",str(config)],capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,0,result.stderr)
            saved = b'{"preserved_prior_attempt": true}\n'
            (frozen / "result.json").write_bytes(saved)
            (frozen / "capture-01").mkdir()
            result = subprocess.run([sys.executable,str(SCRIPT),"capture","--output",str(frozen)],capture_output=True,text=True,timeout=10)
            self.assertNotEqual(result.returncode,0)
            self.assertEqual((frozen / "result.json").read_bytes(),saved)
    def test_malformed_http_json_is_retained_as_invalid_response(self):
        class Model(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers['Content-Length']))
                self.send_response(200);self.send_header('Content-Length','7');self.end_headers();self.wfile.write(b'notJSON')
            def log_message(self,*args):pass
        with one_fixed_capture(Model) as (root,completed):
            self.assertEqual(completed.returncode,0,completed.stderr)
            attempts = [json.loads(line) for line in (root/'capture-01/inference-attempts.jsonl').read_text().splitlines()]
            self.assertEqual(len(attempts),1)
            self.assertEqual(attempts[0]['outcome'],'invalid_response')
            self.assertEqual(attempts[0]['response_raw'],'notJSON')

    def test_redirect_is_refused_before_a_second_http_request(self):
        requests = []
        class Model(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers['Content-Length']));requests.append(self.path)
                if self.path=='/trap':
                    body = json.dumps({'choices':[{'message':{'content':'{}'},'finish_reason':'stop'}]}).encode()
                    self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
                else:
                    self.send_response(302);self.send_header('Location',f'http://127.0.0.1:{self.server.server_port}/trap');self.send_header('Content-Length','0');self.end_headers()
            def do_GET(self):
                requests.append(self.path);self.send_response(200);self.send_header('Content-Length','2');self.end_headers();self.wfile.write(b'{}')
            def log_message(self,*args):pass
        with one_fixed_capture(Model) as (root,completed):
            self.assertEqual(completed.returncode,0,completed.stderr)
            self.assertEqual(requests,['/v1/chat/completions'])
            attempts = [json.loads(line) for line in (root/'capture-01/inference-attempts.jsonl').read_text().splitlines()]
            self.assertEqual(len(attempts),1)
            self.assertEqual(attempts[0]['http_status'],302)

    def test_proxy_environment_cannot_reroute_local_evidence(self):
        proxy_calls = []
        class Proxy(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                proxy_calls.append(self.rfile.read(int(self.headers['Content-Length'])))
                self.send_response(503);self.end_headers()
            def log_message(self,*args):pass
        class Model(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers['Content-Length']))
                answer = {'answer':'Blue','source_ids':['episode:source'],'abstain':False}
                body=json.dumps({'choices':[{'message':{'content':json.dumps(answer)},'finish_reason':'stop'}]}).encode()
                self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
            def log_message(self,*args):pass
        proxy = http.server.ThreadingHTTPServer(('127.0.0.1',0),Proxy)
        worker = threading.Thread(target=proxy.serve_forever,daemon=True);worker.start()
        try:
            env={k:v for k,v in os.environ.items() if k.lower() not in ('http_proxy','https_proxy','all_proxy','no_proxy')}
            env.update(HTTP_PROXY=f'http://127.0.0.1:{proxy.server_port}',HTTPS_PROXY=f'http://127.0.0.1:{proxy.server_port}',NO_PROXY='')
            with one_fixed_capture(Model,environment=env) as (root,completed):
                self.assertEqual(completed.returncode,0,completed.stderr)
                self.assertEqual(proxy_calls,[])
                self.assertTrue(json.loads((root/'capture-01/rows.json').read_text())[0]['scores']['passed'])
        finally:
            proxy.shutdown();proxy.server_close();worker.join(timeout=2)


if __name__ == "__main__":
    unittest.main()
