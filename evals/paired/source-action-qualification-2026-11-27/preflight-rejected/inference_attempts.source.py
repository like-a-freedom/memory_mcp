"""Explicit retry accounting for the local, non-streaming evaluation protocol.

The callback must perform exactly one transport attempt (SDK retries disabled).
First observed request is not proof of cold model state. Server startup and warmup
are recorded by the launching protocol, never inferred from request latency.
"""
import asyncio
from contextlib import contextmanager
from contextvars import ContextVar
import json
import math
import os
import threading
import time
from urllib.error import URLError
from unittest.mock import patch


ACTIVE_CALL = ContextVar('longmemeval_active_call', default=None)


class InvalidResponse(ValueError):
    def __init__(self, message, raw_response):
        super().__init__(message)
        self.raw_response = raw_response


def completion_body(result):
    body = result.model_dump(mode='json') if hasattr(result, 'model_dump') else result
    choices = body.get('choices') if isinstance(body, dict) else None
    if (not isinstance(choices, list) or not choices or not isinstance(choices[0], dict)
            or not isinstance(choices[0].get('message'), dict)
            or not isinstance(choices[0]['message'].get('content'), str)):
        raise InvalidResponse('malformed text completion envelope', body)
    return body


def local_async_client(base_url, api_key):
    """Owned local transport: never follow redirects or inherit proxy routing."""
    from openai import AsyncOpenAI
    import httpx
    from urllib.parse import urlparse
    import ipaddress
    url = urlparse(str(base_url))
    if (url.scheme not in ('http', 'https') or url.username is not None or url.password is not None
            or not ipaddress.ip_address(url.hostname).is_loopback):
        raise ValueError('numeric loopback completion endpoint without credentials required')
    return AsyncOpenAI(base_url=base_url, api_key=api_key, max_retries=0,
                       http_client=httpx.AsyncClient(trust_env=False, follow_redirects=False))


def classify(error):
    name = type(error).__name__
    if isinstance(error, (asyncio.CancelledError, KeyboardInterrupt, SystemExit)):
        return 'cancelled', False
    if isinstance(error, TimeoutError) or name in {'APITimeoutError', 'ReadTimeout', 'ConnectTimeout'}:
        return 'timeout', True
    status = getattr(error, 'status_code', getattr(error, 'code', None))
    if status is not None:
        return 'http_error', status in {408, 409, 429} or status >= 500
    if isinstance(error, (ConnectionError, URLError, OSError)) or name in {'APIConnectionError', 'RemoteDisconnected'}:
        return 'transport_error', True
    return 'invalid_response', False


class AttemptRecorder:
    def __init__(self, output, *, max_retries, timeout_seconds):
        if (type(max_retries) is not int or not 0 <= max_retries <= 10
                or isinstance(timeout_seconds, bool) or not math.isfinite(timeout_seconds)
                or timeout_seconds <= 0):
            raise ValueError('finite positive timeout and retry budget in [0,10] required')
        self.output = output
        self.max_retries = max_retries
        self.timeout_seconds = timeout_seconds
        self.lock = threading.Lock()
        self.logical_counts = {}
        self.requests = 0
        self.responses = {}

    def begin(self, case, role):
        if role not in {'reader', 'judge', 'selector'} or not isinstance(case, str) or not case:
            raise ValueError('registered case and reader/judge/selector role required')
        with self.lock:
            key = (case, role)
            count = self.logical_counts.get(key, 0) + 1
            self.logical_counts[key] = count
        return f'{role}:{case}:{count}'

    def start(self, case, role, logical, attempt):
        with self.lock:
            self.requests += 1
            state = 'first_observed_request' if self.requests == 1 else 'subsequent_request'
            # Write before dispatch. An unmatched start is pending/unobserved,
            # not proof that the peer received a request or that it succeeded.
            self.event({'event':'attempt_started','question_id':case,'role':role,
                        'logical_id':logical,'request_id':f'{logical}:{attempt}',
                        'attempt':attempt,'terminal':False,'outcome':'pending',
                        'temperature_state':state})
            return state

    def event(self, row):
        with (self.output / 'inference-events.jsonl').open('a', encoding='utf-8') as stream:
            stream.write(json.dumps({'schema':'memory-mcp-inference-event/v1',**row},sort_keys=True)+'\n')
            stream.flush()
            os.fsync(stream.fileno())

    def finish(self, case, role, logical, attempt, started, state, result=None, error=None):
        outcome, retryable = classify(error) if error else ('success', False)
        terminal = not retryable or attempt > self.max_retries
        raw = getattr(error, 'raw_response', None) if error else result
        body = raw.model_dump(mode='json') if hasattr(raw, 'model_dump') else raw
        body = body if isinstance(body, dict) else {}
        choices = body.get('choices')
        choice = choices[0] if isinstance(choices,list) and choices and isinstance(choices[0],dict) else {}
        row = {
            'schema': 'memory-mcp-inference-attempt/v1',
            'question_id': case, 'role': role, 'logical_id': logical,
            'request_id': f'{logical}:{attempt}', 'attempt': attempt,
            'max_retries': self.max_retries, 'timeout_seconds': self.timeout_seconds,
            'outcome': outcome, 'terminal': terminal,
            'elapsed_seconds': time.monotonic() - started,
            'temperature_state': state,
            'server_startup_seconds': None, 'prefill_seconds': None, 'generation_seconds': None,
            'usage': body.get('usage'), 'finish_reason': choice.get('finish_reason'),
            'error_type': type(error).__name__ if error else None,
            'http_status': getattr(error, 'status_code', getattr(error, 'code', None)),
        }
        if isinstance(error, InvalidResponse):
            row['response_raw'] = error.raw_response
        with self.lock:
            with (self.output / 'inference-attempts.jsonl').open('a', encoding='utf-8') as stream:
                stream.write(json.dumps(row, sort_keys=True) + '\n')
                stream.flush()
            self.event({'event':'attempt_finished',**{k:v for k,v in row.items() if k != 'schema'}})
            if result is not None:
                self.responses[(case, role)] = row
        return terminal

    def call(self, case, role, request):
        logical = self.begin(case, role)
        for attempt in range(1, self.max_retries + 2):
            state, started = self.start(case,role,logical,attempt), time.monotonic()
            try:
                result = request()
                completion_body(result)
            except (KeyboardInterrupt, SystemExit) as error:
                self.finish(case,role,logical,attempt,started,state,error=error)
                raise
            except Exception as error:
                if self.finish(case, role, logical, attempt, started, state, error=error):
                    raise
            else:
                self.finish(case, role, logical, attempt, started, state, result=result)
                return result

    async def call_async(self, case, role, request):
        logical = self.begin(case, role)
        for attempt in range(1, self.max_retries + 2):
            state, started = self.start(case,role,logical,attempt), time.monotonic()
            try:
                async with asyncio.timeout(self.timeout_seconds):
                    result = await request()
                    completion_body(result)
            except asyncio.CancelledError as error:
                self.finish(case, role, logical, attempt, started, state, error=error)
                raise
            except Exception as error:
                if self.finish(case, role, logical, attempt, started, state, error=error):
                    raise
            else:
                self.finish(case, role, logical, attempt, started, state, result=result)
                return result


@contextmanager
def observed_sdk_clients(harness, recorder):
    """Adapt existing upstream clients without altering the pinned upstream files."""
    from evaluation import qa_eval_metrics
    async_factory = harness.create_async_client
    sync_factory = qa_eval_metrics._create_openai_client

    def wrap(client, asynchronous):
        client.max_retries = 0

        def parameters(kwargs):
            identity = ACTIVE_CALL.get()
            if identity is None:
                raise ValueError('inference request lacks case/role identity')
            kwargs['timeout'] = recorder.timeout_seconds
            return identity, kwargs

        async def async_create(**kwargs):
            (case, role), kwargs = parameters(kwargs)
            async with local_async_client(str(client.base_url),client.api_key) as transport:
                return await recorder.call_async(case, role, lambda: transport.chat.completions.create(**kwargs))

        def sync_create(**kwargs):
            (case, role), kwargs = parameters(kwargs)
            # The upstream judge is synchronous. Run its one completion through
            # the async transport so the deadline bounds the whole response,
            # including a peer that keeps trickling bytes within read timeouts.
            async def complete():
                async with local_async_client(str(client.base_url),client.api_key) as transport:
                    return await recorder.call_async(
                        case, role, lambda: transport.chat.completions.create(**kwargs))
            try:
                return asyncio.run(complete())
            finally:
                client.close()

        client.chat.completions.create = async_create if asynchronous else sync_create
        return client

    with (patch.object(harness, 'create_async_client', lambda *a, **kw: wrap(async_factory(*a, **kw), True)),
          patch.object(qa_eval_metrics, '_create_openai_client', lambda *a, **kw: wrap(sync_factory(*a, **kw), False))):
        yield
