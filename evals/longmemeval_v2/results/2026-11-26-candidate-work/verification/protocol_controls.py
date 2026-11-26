"""Public script controls; local subprocess/file inputs, no model invocation."""
from pathlib import Path
import hashlib,json,os,shutil,subprocess,sys,tempfile,unittest
REPO=Path.cwd();BASE=REPO/'evals/longmemeval_v2/results/2026-11-26-candidate-work';FINAL=BASE/'original-seven-post-topical'
def save(p,x):p.parent.mkdir(parents=True,exist_ok=True);p.write_text(json.dumps(x))
def load(p):return json.loads(p.read_text())
class ProtocolControls(unittest.TestCase):
 def gate_control(self, mismatch=False, changed=False):
  with tempfile.TemporaryDirectory() as t:
   repo=Path(t);base=repo/'artifacts';root=base/'current';root.mkdir(parents=True)
   shutil.copy2(FINAL/'capture.py',root/'capture.py');save(repo/'evals/longmemeval_v2/small-web-native-profile.json',{})
   save(base/'verification/topical-tabs-current/commands.json',[{'name':n,'exit_code':0} for n in ['workspace-clippy','diagnostic-clippy','fmt','diff','scoped-semantic','production','harness']])
   save(base/'native-gate-topical/comparison.json',{'all_cases_pass':True,'no_regressions':True,'counts':{'passed':8,'failed':0,'invalid':0,'unobserved':0}})
   binary=repo/'binary';binary.write_bytes(b'controlled binary');sha=hashlib.sha256(binary.read_bytes()).hexdigest()
   save(base/'verification/topical-tabs-current/release-build.json',{'binary':str(binary),'exit_code':0,'sha256':sha})
   save(base/'native-gate-topical/registration.json',{'product_binary_sha256':'mismatched' if mismatch else sha})
   save(base/'native-gate-topical/binary-integrity.json',{'product_unchanged':not changed,'runner_unchanged':True})
   save(base/'topical-tabs-final/registration.json',{'candidate_source_sha256':{}})
   r=subprocess.run([sys.executable,str(root/'capture.py')],cwd=repo,capture_output=True,text=True,timeout=10)
   self.assertNotEqual(r.returncode,0)
   self.assertIn('native gate must qualify the exact unchanged release binary',r.stderr)
 def test_rejects_a_native_gate_for_another_release(self):self.gate_control(mismatch=True)
 def test_rejects_changed_gate_binary(self):self.gate_control(changed=True)
 def partial_control(self, reader_error=False):
  with tempfile.TemporaryDirectory() as t:
   base=Path(t);root=base/'current';root.mkdir()
   (base/'original-seven-post-container').symlink_to(BASE/'original-seven-post-container',target_is_directory=True)
   shutil.copy2(FINAL/'analyze.py',root/'analyze.py');shutil.copytree(BASE/'original-seven-post-container/scores',root/'scores')
   for name in ['analysis-registration.json','final-integrity.json']:
    shutil.copy2(FINAL/name if (FINAL/name).exists() else BASE/'original-seven-post-container'/name,root/name)
   p=root/'scores/per_question.jsonl';rows=[json.loads(l) for l in p.read_text().splitlines()];rows=[r for r in rows if r['question_id']!='0c6b0c60'];p.write_text(''.join(json.dumps(r)+'\n' for r in rows))
   if reader_error:
    p=root/'scores/reader_outputs.jsonl';rows=[json.loads(l) for l in p.read_text().splitlines()]
    rows=[{**r,'status':'timeout','response_raw':None} if r['question_id']=='0c6b0c60' else r for r in rows];p.write_text(''.join(json.dumps(r)+'\n' for r in rows))
   coverage=load(root/'scores/score-coverage.json');coverage.update(native_scored_case_count=6,covered_case_count=6,coverage_adjusted_success_rate=2/7);coverage['cases']['0c6b0c60']='reader_timeout' if reader_error else 'judge_error';save(root/'scores/score-coverage.json',coverage)
   r=subprocess.run([sys.executable,str(root/'analyze.py')],cwd=REPO,capture_output=True,text=True,timeout=30)
   self.assertNotEqual(r.returncode,0,'incomplete capture cannot pass final acceptance')
   self.assertTrue((root/'partial-analysis.json').exists(),r.stderr)
   report=load(root/'partial-analysis.json');self.assertEqual(report['selected'],7);self.assertFalse(report['acceptance_complete']);self.assertEqual(report['source_presence']['selected'],7);self.assertEqual(report['source_presence']['present'],4);self.assertEqual(report['native_scored_cases'],6)
   self.assertEqual(report['cases']['0c6b0c60']['native_status'],coverage['cases']['0c6b0c60'])
 def test_judge_failure_retains_all_prompt_source_measurements(self):self.partial_control()
 def test_reader_failure_retains_all_prompt_source_measurements(self):self.partial_control(reader_error=True)
 def test_complete_capture_preserves_the_original_metrics(self):
  with tempfile.TemporaryDirectory(dir=BASE/'verification') as t:
   base=Path(t);root=base/'current';root.mkdir()
   (base/'original-seven-post-container').symlink_to(BASE/'original-seven-post-container',target_is_directory=True)
   (base/'topical-tabs-final').symlink_to(BASE/'topical-tabs-final',target_is_directory=True)
   shutil.copy2(FINAL/'analyze.py',root/'analyze.py');shutil.copytree(BASE/'original-seven-post-container/scores',root/'scores')
   shutil.copy2(FINAL/'analysis-registration.json',root/'analysis-registration.json')
   for name in ['final-integrity.json','registration.json']:
    shutil.copy2(BASE/'original-seven-post-container'/name,root/name)
   r=subprocess.run([sys.executable,str(root/'analyze.py')],cwd=REPO,capture_output=True,text=True,timeout=30)
   self.assertEqual(r.returncode,0,r.stderr)
   report=load(root/'final-analysis.json')
   self.assertEqual(report['strict_independent_answers']['correct'],3)
   self.assertEqual(report['strict_independent_answers']['selected'],7)
   self.assertEqual(report['source_presence_v2']['candidate']['present'],4)
if __name__=='__main__':unittest.main()
