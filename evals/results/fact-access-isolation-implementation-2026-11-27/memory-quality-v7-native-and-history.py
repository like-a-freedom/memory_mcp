from pathlib import Path
from datetime import datetime,timezone
import hashlib,importlib.util,json,platform,subprocess,shutil
ROOT=Path('/Users/solovey/Documents/dev/memory_mcp');BASE=ROOT/'evals/longmemeval_v2/results/2026-11-27-full-history-access-isolation';OUT=ROOT/'evals/longmemeval_v2/results/2026-11-27-full-history-positional-v7'
spec=importlib.util.spec_from_file_location('capture_v7','/tmp/memory-quality-v7-source-capture.py');d=importlib.util.module_from_spec(spec);spec.loader.exec_module(d);h=d.h
pins=h.load(ROOT/'evals/results/fact-access-isolation-implementation-2026-11-27/binary-pins-v7.json')
for row in pins['binaries']:
 if h.sha(Path(row['path']))!=row['sha256']:raise RuntimeError('pinned binary changed')
if platform.machine()!='arm64':raise RuntimeError('native Apple Silicon required for this capture')
OUT.mkdir(exist_ok=False)
files=[Path('/tmp/memory-quality-v7-source-capture.py'),d.HELPER,h.BUILD_INPUT,h.READ_INPUT,h.RETENTION,h.VERIFIER,BASE/'snapshot-admission.json',ROOT/'Cargo.lock',d.BIN]
files+=list((ROOT/'crates/memory-mcp/src/memory').rglob('*.rs'))
files+=[ROOT/'crates/memory-mcp/tests/full_history_evidence.rs',ROOT/'crates/memory-mcp/tests/long_history_stage_evidence.rs']
expected_source={name:meta['sha256'] for name,meta in h.load(h.RETENTION)['files'].items()};expected_candidate=h.load(BASE/'snapshot-admission.json')['upgraded_file_sha256']
if h.hashes(h.SOURCE)!=expected_source or h.hashes(h.UPGRADED)!=expected_candidate:raise RuntimeError('frozen source or upgraded snapshot changed')
registration={'schema':'full-history-positional-controls/v1','registered_at_utc':datetime.now(timezone.utc).isoformat(),'registered_before_outputs':True,'architecture':'arm64','candidate':'v7','superseded_build_only':'v6 binaries pinned but no native/full-history/reader outputs produced; independent quote-dispute RED found before dispatch and fixed in v7','intervention':'Only explicit UI control noun bound to between two distinct quoted labels; prior membership rule unchanged; no changed cases, labels, budgets, policy or time rules','cases':list(h.CASES),'scheduled_reads':7,'repetitions':1,'retry':0,'limits':h.LIMITS,'frozen_sha256':{str(p):h.sha(p) for p in files},'source_snapshot_file_sha256':expected_source,'candidate_snapshot_file_sha256':expected_candidate,'acceptance':{'runtime_completed':7,'read_deadlines':0,'strict_original_source_span_min':4,'independently_sufficient_actual_sources_min':7,'native_temporal_pass':8,'native_retrieval_pass':37,'native_invalid':0,'original_six_native_floors':1.0},'comparison':'All7 v5 candidate receipts and paired baseline retained unchanged; this is a new source-selection version, not a replacement retry or a paired speed study','binary_pins':pins}
h.save(OUT/'registration.json',registration);shutil.copy2('/tmp/memory-quality-v7-source-capture.py',OUT/'capture_driver.source.py')
# Register native commands before either evaluation starts.
subprocess.run(['python3','/tmp/memory-quality-v7-registered-gates.py'],cwd=ROOT,check=True,timeout=30)
for role,cmd,deadline in [('native',['python3','/tmp/memory-quality-v7-registered-gates.py','run'],1000),('history',['python3','/tmp/memory-quality-v7-source-capture.py'],600)]:
 print('starting',role,flush=True)
 r=subprocess.run(cmd,cwd=ROOT,timeout=deadline)
 if r.returncode:raise SystemExit(r.returncode)
