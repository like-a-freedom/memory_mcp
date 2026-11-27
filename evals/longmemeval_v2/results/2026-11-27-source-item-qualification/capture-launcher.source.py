"""One sequential native capture with a single outer 1800-second budget."""
import asyncio
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import platform
import sys
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT))
from evals.longmemeval_v2.reader_protocol import capture

OUT = Path(__file__).resolve().parent


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


async def main():
    started = time.monotonic()
    acceptance = json.loads((OUT/'acceptance.json').read_text())
    assert platform.system() == 'Darwin' and platform.machine() == 'arm64'
    for cohort, identity in acceptance['registrations'].items():
        assert sha(OUT/cohort/'registration.json') == identity
    prior = ROOT/'evals/paired/source-action-qualification-2026-11-27'
    assert (prior/'capture-completed.json').is_file(), 'no overlapping workflow inference'
    identity = json.loads((prior/'execution-identity.json').read_text())
    pid = int(Path('/tmp/memory-quality-owned-mlx-reader.pid').read_text())
    assert pid == identity['owned_server_pid']
    health = json.load(urllib.request.urlopen('http://127.0.0.1:18767/v1/models', timeout=5))
    model = json.loads((OUT/'shared40/registration.json').read_text())['model']
    assert health['data'][0]['id'] == model
    with (OUT/'execution-identity.json').open('x') as target:
        json.dump({'at_utc':datetime.now(timezone.utc).isoformat(),
                   'acceptance_sha256':sha(OUT/'acceptance.json'),
                   'runtime_identity_path':str(prior/'execution-identity.json'),
                   'runtime_identity_sha256':sha(prior/'execution-identity.json'),
                   'model':model, 'owned_server_pid':pid,
                   'scope':'native local offline MLX; no LM Studio service; sequential calls',
                   'scheduled':72, 'global_budget_seconds':1800}, target, indent=2)
    outcome = 'completed'
    try:
        for cohort in ('shared40', 'fresh16'):
            remaining = 1800-(time.monotonic()-started)
            if remaining <= 0:
                raise TimeoutError('global budget exhausted')
            await asyncio.wait_for(capture(OUT/cohort), timeout=remaining)
    except Exception as error:
        outcome = type(error).__name__
        raise
    finally:
        with (OUT/'capture-terminal.json').open('x') as target:
            json.dump({'outcome':outcome,'elapsed_seconds':time.monotonic()-started,
                       'scheduled':72,'missing_outputs':'fail in fixed denominator; no retry'},target,indent=2)


if __name__ == '__main__':
    asyncio.run(main())
