"""Replayable local tool-using evaluation of the frozen Memory MCP and CLI skills."""

import argparse
import asyncio
from contextlib import contextmanager
from datetime import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
from tempfile import TemporaryDirectory
from threading import Timer
import time

from evals.longmemeval_v2.inference_attempts import AttemptRecorder, completion_body, local_async_client
from evals.longmemeval_v2.scored_coverage import loopback_endpoint
from evals.paired.memory_quality_diagnostic import MCP


CLI_COMMANDS = frozenset({'ingest', 'extract', 'assemble-context', 'explain', 'resolve',
                          'invalidate', 'import', 'reembed'})


class SkillHost:
    """The only file and tool boundary available to a skill participant."""

    def __init__(self, skill, references, tools, cli=None, *, tool_schemas=None):
        self.skill = skill
        self.references = references
        self.tools = tools
        self.cli = cli
        if tool_schemas is not None and set(tool_schemas) != set(tools):
            raise ValueError('schema catalog must contain every live tool and no others')
        self.tool_schemas = {} if tool_schemas is None else tool_schemas
        self.trace = []

    def dispatch(self, request):
        record = {'request': request, 'response': None}
        self.trace.append(record)
        try:
            response = self.execute(request)
        except (KeyError, TypeError, ValueError) as error:
            response = {'error': str(error), 'denied': True}
        except Exception as error:
            record['response'] = {'error': str(error), 'host_error': type(error).__name__}
            raise
        record['response'] = response
        return response

    def execute(self, request):
        match request:
            case {'action': 'describe_tool', 'name': str(name)} if len(request) == 2:
                return {'name': name, 'inputSchema': self.tool_schemas[name]}
            case {'action': 'read_reference', 'path': str(path)} if len(request) == 2:
                return {'text': self.references[path]}
            case {'action': 'call', 'name': str(name), 'arguments': dict(arguments)} if len(request) == 3:
                return self.tools[name](arguments)
            case {'action': 'run', 'argv': ['memory_mcp', command, *rest]} if self.cli is not None:
                if request.keys() - {'action', 'argv', 'stdin'} or command not in CLI_COMMANDS:
                    raise ValueError('unsupported CLI command')
                stdin = request.get('stdin', '')
                if not all(isinstance(value, str) and '\0' not in value for value in rest) or not isinstance(stdin, str):
                    raise ValueError('argv and stdin must contain text')
                return self.cli(request['argv'], stdin)
            case _:
                raise ValueError('unsupported action')


def canonical_id(value, table):
    return isinstance(value, str) and value.startswith(table + ':') and bool(value.split(':', 1)[1].strip())


def capture_status(envelope, *, expected_episode=None, exit_status=0):
    result = envelope.get('result')
    if (exit_status != 0 or envelope.get('status') != 'success'
            or 'extraction is pending' in str(envelope.get('guidance', '')).casefold()
            or not isinstance(result, dict) or not isinstance(result.get('facts'), list)):
        return 'pending'
    episode = result.get('episode_id')
    if (not canonical_id(episode, 'episode') or expected_episode is not None and episode != expected_episode
            or any(not isinstance(fact, dict) or not canonical_id(fact.get('fact_id'), 'fact')
                   for fact in result['facts'])):
        return 'pending'
    return 'verified' if result['facts'] else 'episode-only'


ROOT = Path(__file__).resolve().parents[2]
REGISTRATION = ROOT / 'evals/paired/skill-qualification-2026-11-27'
OBSERVER_SOURCE = ROOT / 'evals/longmemeval_v2/inference_attempts.py'
MCP_BINDING_SOURCE = ROOT / 'evals/paired/memory_quality_diagnostic.py'
ARMS = ('previous', 'current')
OPERATION_PROTOCOL = 'memory-mcp-skill-operations/v2'
MCP_HOST = 'mcp__memory__'
NATIVE = {('memory-mcp', 1), ('memory-mcp', 2), ('memory-mcp', 3), ('memory-mcp', 4),
          ('memory-cli', 1), ('memory-cli', 2), ('memory-cli', 4)}
BUDGET = {'turns': 9, 'max_tokens': 384, 'call_seconds': 20, 'case_seconds': 180,
          'retries': 0, 'temperature': 0}
PRODUCT_ENV = {'EMBEDDINGS_ENABLED': 'false', 'NER_EXTRACTOR': 'regex',
               'RUST_LOG': 'error', 'MEMORY_CLAIM_ROLLOUT_STAGE': 'evidence',
               'HF_HUB_OFFLINE': '1', 'TRANSFORMERS_OFFLINE': '1'}

SOURCE = {
    'source_type': 'conversation', 'source_id': 'cedar-release-minutes-2026-11-20',
    't_ref': '2026-11-20T09:30:00Z',
    'content': 'Decision: Cedar Index publishes incremental updates every Tuesday.',
}
RECALL_SOURCE = {
    'source_type': 'conversation', 'source_id': 'cedar-index-minutes-2026-11-18',
    't_ref': '2026-11-18T09:00:00Z',
    'content': 'Decision: The Cedar Index uses weekly incremental indexing.',
}
LITERAL = {
    'source_type': 'note', 'source_id': 'literal-spec-references-2026-11-23',
    't_ref': '2026-11-23T11:15:00Z',
    'content': 'https://example.org/spec\n/work/spec.md',
}
SNAPSHOT = {'valid_at': '2026-10-20T12:00:00Z', 'known_revision': 17,
            'known_at': '2026-11-23T10:00:00Z'}
SELECTORS = {'query': 'Cedar Index retention window', 'fact_types': ['decision'],
             'view_mode': 'current', 'budget': 3}
PARTIAL_READ = {
    'status': 'partial', 'result': [],
    'context': {'snapshot': SNAPSHOT, 'completeness': 'partial',
                'omitted_channels': ['semantic ANN and mutable embeddings'],
                'candidate_scan_truncated': False},
    'guidance': 'At most one refinement. Preserve the returned valid_at and known_revision.',
}
PENDING = {
    'status': 'partial', 'result': {'episode_id': 'episode:pending-cedar-295', 'facts': [],
                                   'warnings': ['Extraction did not finish.']},
    'guidance': 'Extraction is pending. Retry is safe using the original episode_id.',
}
LONG_EPISODE = 'episode:cedar-long-report'
LONG_FACT = 'fact:cedar-long-approval'
LONG_SOURCE = 'cedar-retention-report-v3'
QUOTE = 'Cedar Index retains exactly 42 full days after a successful rollover.'
LONG_TEXT = '🪶' * 1200 + QUOTE + '\n' + 'Appendix. ' * 70
LONG_ITEM = {'fact_id': LONG_FACT, 'content': QUOTE, 'source_episode': LONG_EPISODE,
             'quote': QUOTE, 'provenance': {'source_id': LONG_SOURCE}}
EXTRACTION_OMITTED = {
    'status': 'success',
    'result': {'episode_id': LONG_EPISODE, 'facts': [{'fact_id': LONG_FACT, 'type': 'decision'}],
               'fact_evidence_omitted': True, 'warnings': []},
    'guidance': 'Use explain to verify evidence.',
}
IMPORTS = {
    'file': {'root': '/work/notes', 'path': 'doc.md', 'source_type': 'document',
             't_ref': '2026-11-19T08:00:00Z'},
    'url': {'url': 'https://example.org/cedar/public-notes', 'source_type': 'web',
            't_ref': '2026-11-21T08:00:00Z'},
}


def envelope(status, result, guidance=''):
    return {'status': status, 'result': result, 'guidance': guidance}


def process_result(result, exit_status=0, stderr=''):
    return {'stdout': json.dumps(result, ensure_ascii=False), 'stderr': stderr,
            'exit_status': exit_status}


def fixture(skill, case_id):
    """Provide only missing user/source/prior response inputs, never hidden labels."""
    if case_id == 1:
        return {'confirmed_source_message': SOURCE}
    if case_id == 2:
        return {'topic_name': 'Cedar Index'}
    if (skill, case_id) == ('memory-mcp', 3):
        return {'confirmed_identity_type': 'person', 'canonical_display_name': 'Alice Smith'}
    if (skill, case_id) == ('memory-cli', 3):
        return {'embedding_model_changed': True, 'previous_failed_job': False,
                'operator_preference': 'Abort on the first failed fact. Report terminal status.'}
    if case_id == 4:
        return {'literal_source_message': LITERAL}
    if (skill, case_id) in {('memory-mcp', 5), ('memory-cli', 6)}:
        prior = {'previous_selectors': {**SELECTORS, 'valid_at': SNAPSHOT['valid_at'],
                                       'known_at': SNAPSHOT['known_at']},
                 'previous_response': PARTIAL_READ}
        if skill == 'memory-cli':
            prior['previous_explain'] = {
                'item': LONG_ITEM, 'response': long_explain(LONG_ITEM, 0, 0, 1200, compact=False),
            }
        return prior
    if (skill, case_id) == ('memory-mcp', 6):
        return {'previous_extract': EXTRACTION_OMITTED}
    if (skill, case_id) == ('memory-cli', 5):
        return {'authorized_public_page': IMPORTS['url']['url'],
                'doc_md_event_time': IMPORTS['file']['t_ref'],
                'public_page_event_time': IMPORTS['url']['t_ref'],
                'operator_note': 'Check the commands in the available disposable bindings before reporting them.'}
    if (skill, case_id) == ('memory-mcp', 7):
        return {'first_extract': envelope('success', {
                    'episode_id': 'episode:cedar-freeform-note',
                    'facts': [{'fact_id': 'fact:cedar-freeform-note', 'type': 'note'}]}),
                'second_extract': envelope('success', {
                    'episode_id': 'episode:cedar-freeform-empty', 'facts': []})}
    if (skill, case_id) == ('memory-cli', 7):
        return {'previous_extract': process_result(EXTRACTION_OMITTED)}
    if case_id == 8:
        return {'previous_extract': process_result(PENDING, stderr='Extraction work is incomplete.')}


def long_explain(item, source_offset, provenance_offset, limit, *, compact=True):
    quote = item.get('quote', '')
    anchor = LONG_TEXT.find(quote) if quote else -1
    explicit = source_offset is not None
    start = source_offset if explicit else max(0, anchor - 600)
    limit = limit if limit is not None else 8000 if explicit else 1200
    excerpt = LONG_TEXT[start:start + limit]
    end = start + len(excerpt)
    next_source = end if end < len(LONG_TEXT) else None
    next_provenance = 20 if provenance_offset == 0 else None
    reasons = []
    if explicit and next_source is not None:
        reasons.append('source_window_incomplete')
    if next_provenance is not None:
        reasons.append('provenance_truncated')
    if quote and anchor < 0:
        reasons.append('quote_unverified')
    response = {**item, 'provenance': {'source_id': LONG_SOURCE},
                'quote_verified': anchor >= 0, 'quote_complete': bool(quote),
                'quote_supplied': bool(quote), 'quote_returned': bool(quote),
                'quote_anchored': anchor >= 0 and not explicit,
                'quote_contained_in_window': start <= anchor and anchor + len(quote) <= end,
                'citation_context': excerpt, 'citation_context_exact': True,
                'citation_context_covers_entire_source': start == 0 and end == len(LONG_TEXT),
                'citation_context_start_offset': start,
                'citation_context_scalars_returned': len(excerpt),
                'citation_context_scalars_total': len(LONG_TEXT),
                'provenance_next_offset': next_provenance, 'provenance_total_count': 21,
                'partial_reasons': reasons}
    if explicit:
        response['citation_context_next_offset'] = next_source
    if quote or not compact:
        response['quote'] = quote
    else:
        response.pop('quote', None)
    return envelope('partial' if reasons else 'success', [response],
                    'Use the returned page offsets for the selected item.')


def cli_options(args):
    """Decode the public CLI shape for validation and observation only."""
    options = {}
    markers = {'--content-stdin', '--compact', '--retry-failed'}
    while args:
        flag, *args = args
        if not flag.startswith('--'):
            raise ValueError('expected a named CLI flag')
        flag, separator, value = flag.partition('=')
        if flag in markers:
            if separator:
                raise ValueError('unexpected value for a CLI switch')
            value = True
        elif not separator:
            if not args or args[0].startswith('--'):
                raise ValueError('missing CLI option value')
            value, *args = args
        key = flag[2:].replace('-', '_')
        if key in {'known_revision', 'budget', 'source_offset', 'source_limit_scalars',
                   'item_offset', 'provenance_offset', 'max_failures'}:
            if not value.isascii() or not value.isdecimal():
                raise ValueError(f'{flag} requires a natural number')
            value = int(value)
        if key == 'fact_type':
            options.setdefault('fact_types', []).append(value)
        elif key in options:
            raise ValueError('duplicate CLI option')
        else:
            options[key] = value
    return options


def exact_history(args):
    expected = {**SELECTORS, 'valid_at': SNAPSHOT['valid_at'],
                'known_revision': SNAPSHOT['known_revision']}
    return all(type(args.get(key)) is int for key in ('known_revision', 'budget')) and args == expected


def selected_long_item(args):
    fields = {'context_items', 'compact', 'source_offset', 'source_limit_scalars',
              'provenance_offset', 'item_offset'}
    offsets = {'source_offset', 'source_limit_scalars', 'provenance_offset', 'item_offset'}
    if (set(args) - fields or 'compact' in args and type(args['compact']) is not bool
            or any(type(args[key]) is not int or args[key] < 0 for key in offsets & args.keys())
            or 'source_limit_scalars' in args and 'source_offset' not in args):
        return None
    try:
        values = json.loads(args['context_items'])
        if not isinstance(values, list):
            return None
        if len(values) > 1 and 'item_offset' not in args:
            return None
        item = values[args.get('item_offset', 0)]
        if item == LONG_EPISODE:
            return {'source_episode': item}
        if (isinstance(item, dict) and item.get('source_episode') == LONG_EPISODE
                and item.get('fact_id', LONG_FACT) == LONG_FACT
                and not item.keys() & {'id', 'factId', 'sourceEpisode', 'citationContext', 'allSources',
                                      'graphInsights', 'tRef', 'tIngested'}
                and all(isinstance(item[key], str) for key in ('content', 'quote') if key in item)):
            return {key: item[key] for key in ('content', 'quote', 'source_episode', 'fact_id') if key in item}
    except (KeyError, IndexError, ValueError, TypeError):
        pass
    return None


class Controlled:
    """Deterministic synthetic scenario backend. No processes, reads or egress."""

    def __init__(self, skill, case_id):
        self.skill = skill
        self.case_id = case_id
        self.imported = []

    def mcp(self, name, args):
        if name == 'assemble_context' and (self.skill, self.case_id) in {
                ('memory-mcp', 5), ('memory-cli', 6)}:
            if exact_history(args):
                return {**PARTIAL_READ, 'guidance': 'Refinement exhausted. Report partial coverage.'}
            return envelope('error', None, 'The requested historical boundary or selectors do not match.')
        if name == 'explain' and (self.skill, self.case_id) in {('memory-mcp', 6), ('memory-cli', 6)}:
            item = selected_long_item(args)
            if item is not None:
                offset = args.get('source_offset')
                provenance = args.get('provenance_offset', 0)
                limit = args.get('source_limit_scalars')
                if ((offset is None or offset <= len(LONG_TEXT)) and provenance <= 21
                        and (limit is None or limit <= 8000 and (limit > 0 or offset == len(LONG_TEXT)))):
                    return long_explain(item, offset, provenance, limit, compact=args.get('compact', True))
            return envelope('error', None, 'No matching selected source page.')
        if name == 'extract' and self.case_id == 8 and args == {'episode_id': PENDING['result']['episode_id']}:
            return PENDING
        if name == 'extract' and (self.skill, self.case_id) == ('memory-cli', 7):
            if args == {'episode_id': LONG_EPISODE, 'compact': True}:
                return envelope('success', {key: value for key, value in EXTRACTION_OMITTED['result'].items()
                                            if key != 'fact_evidence_omitted'})
        return envelope('error', None, 'No controlled response for this operation and arguments.')

    def cli(self, argv, stdin):
        command = argv[1]
        if command == 'import' and (len(argv) < 3 or argv[2] not in IMPORTS):
            return process_result(envelope('error', None, 'Select the file or URL import command.'), 2)
        try:
            args = cli_options(argv[3:] if command == 'import' else argv[2:])
        except ValueError as error:
            return process_result(envelope('error', None, str(error)), 2)
        if stdin or command == 'explain' and 'compact' in args:
            return process_result(envelope('error', None, 'Unsupported input for the controlled CLI operation.'), 2)
        if command == 'reembed' and self.case_id == 3:
            if args == {'max_failures': 0}:
                return process_result({'status': 'interrupted', 'processed': 2, 'total': 5,
                                       'failed_count': 1, 'failed_fact_ids': ['fact:cedar-vector-4']},
                                      130, 'Reembedding interrupted by operator.')
        if command == 'import' and self.case_id == 5:
            kind = argv[2]
            if args == IMPORTS.get(kind) and not stdin:
                self.imported.append(kind)
                return process_result(envelope('success', {
                    'episode_id': f'episode:cedar-import-{kind}',
                    'source_id': f'import:{kind}:cedar-notes',
                }, 'Extract stored episode separately.'))
        if command == 'extract' and self.case_id == 5:
            for kind in self.imported:
                if args == {'episode_id': f'episode:cedar-import-{kind}'}:
                    return process_result(envelope('success', {
                        'episode_id': args['episode_id'],
                        'facts': [{'fact_id': f'fact:cedar-import-{kind}', 'type': 'note'}],
                    }))
        result = self.mcp(command.replace('-', '_'), args)
        return process_result(result, 2 if result['status'] == 'error' else 0)


def read_json(path):
    return json.loads(path.read_text(encoding='utf-8'))


def write_json(path, body):
    with path.open('x', encoding='utf-8') as target:
        json.dump(body, target, indent=2, ensure_ascii=False, sort_keys=True)
        target.write('\n')


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def load_registration():
    registration = read_json(REGISTRATION / 'registration.json')
    cases = []
    skills = {}
    for skill in ('memory-mcp', 'memory-cli'):
        identity = read_json(REGISTRATION / f'{skill}-identities.json')
        prompts = (ROOT / f'skills/{skill}/evals/evals.json').read_bytes()
        if sha256(prompts) != identity['prompts_sha256']:
            raise ValueError(f'{skill} prompt identity changed')
        registered = [row for row in registration['cases'] if row['skill'] == skill]
        for prompt, row in zip(json.loads(prompts)['evals'], registered, strict=True):
            if (prompt['id'], prompt['prompt'], prompt['expected_output']) != (
                    row['id'], row['prompt'], row['expected_behavior']):
                raise ValueError(f'{skill} registered prompt or objective differs from the pin')
            cases.append({'skill': skill, 'id': row['id'], 'prompt': row['prompt'],
                          'expected_behavior': row['expected_behavior'],
                          'fixture': fixture(skill, row['id']),
                          'binding': 'native' if (skill, row['id']) in NATIVE else 'controlled'})
        versions = {}
        for arm in ARMS:
            files = {}
            for entry in identity['files']:
                path = entry['path']
                data = ((ROOT / path).read_bytes() if arm == 'current' else
                        subprocess.check_output(['git', 'show', f"{identity['previous_revision']}:{path}"], cwd=ROOT))
                if sha256(data) != entry[f'{arm}_sha256']:
                    raise ValueError(f'{arm} {path} identity changed')
                files[path.removeprefix(f'skills/{skill}/')] = data.decode()
            versions[arm] = files
        skills[skill] = versions
    if len(cases) != registration['denominator'] or len({(c['skill'], c['id']) for c in cases}) != len(cases):
        raise ValueError('case coverage differs from pre-registration')
    return cases, skills


CASE_COHORTS = ('legacy', 'fresh-native/v1')


def fresh_native_cases():
    """New authored inputs, registered before outputs; no held-out benefit claim."""
    cases = []
    for skill in ('memory-mcp', 'memory-cli'):
        for offset, (name, objective, timed) in enumerate((('Maple', 1, False),
                                                        ('Willow', 2, False),
                                                        ('Juniper', 2, True)), 1):
            source = {'source_type':'conversation',
                      'source_id':f'{name.lower()}-schedule-reviewed-2026-11-12',
                      't_ref':'2026-11-12T09:15:00Z',
                      'content':f'Decision: {name} Index publishes incremental updates every Thursday.'}
            selectors = {'valid_at':'2026-11-13T12:00:00Z',
                         'known_at':'2099-11-27T18:00:00Z'} if timed else {}
            inputs = ({'confirmed_source_message':source} if objective == 1
                      else {'topic_name':f'{name} Index', **({'required_time_boundary':selectors} if timed else {})})
            prompt = (f'Capture this confirmed source into memory, extract it, and report the observed outcome.'
                      if objective == 1 else
                      f'Recall the indexing decision for {name} Index and verify its original source using explain. '
                      + ('Use exactly the provided valid and known time boundary. ' if timed else '')
                      + 'Report the decision, its actual source ID and any limitations.')
            cases.append({'skill':skill,'id':100+offset,'objective_id':objective,'binding':'native',
                          'source':source,'fixture':inputs,'prompt':prompt,
                          'required_selectors':selectors,
                          'expected_behavior':('Observed ingest then extract of the exact confirmed source; '
                              'report real episode/status and never turn pending into verified.' if objective == 1
                              else 'Observed recall then explain of a fact from the seeded source; report the exact '
                              'Thursday decision with real source binding and preserve every requested time selector.')})
    return cases


def remaining(deadline, maximum):
    seconds = maximum if deadline is None else min(maximum, deadline - time.monotonic())
    if seconds <= 0:
        raise TimeoutError('trial deadline exhausted')
    return seconds


class DeadlineMCP(MCP):
    """Apply the case deadline to the existing blocking stdio transport."""

    def __init__(self, plan, directory, deadline):
        self.deadline = deadline
        super().__init__(plan, directory)

    def request(self, method, params):
        seconds = remaining(self.deadline, 30)
        timer = Timer(seconds, self.process.kill)
        timer.daemon = True
        timer.start()
        try:
            return super().request(method, params)
        finally:
            timer.cancel()
            timer.join()


def native_cli(binary, store, argv, stdin='', help_only=False, deadline=None):
    env = {name: os.environ[name] for name in ('PATH', 'TMPDIR') if name in os.environ}
    env.update(PRODUCT_ENV)
    env.update({'HOME': str(store.parent), 'SURREALDB_URL': f'surrealkv://{store}'})
    if help_only:
        env.pop('SURREALDB_URL')
    completed = subprocess.run([binary, *argv[1:]], input=stdin.encode(), env=env,
                               capture_output=True, timeout=remaining(deadline, 20), cwd=store.parent)
    return {'stdout': completed.stdout.decode('utf-8', errors='replace'),
            'stderr': completed.stderr.decode('utf-8', errors='replace'),
            'exit_status': completed.returncode}


def cli_ingest_args(source, stdin=False):
    argv = ['memory_mcp', 'ingest', '--source-type', source['source_type'],
            '--source-id', source['source_id'], '--t-ref', source['t_ref']]
    return argv + (['--content-stdin'] if stdin else ['--content', source['content']])


def seed(store, binary, deadline, source=RECALL_SOURCE):
    first = native_cli(binary, store, cli_ingest_args(source, stdin=True), source['content'],
                       deadline=deadline)
    receipt = json.loads(first['stdout'])
    episode = receipt['result']
    second = native_cli(binary, store, ['memory_mcp', 'extract', '--episode-id', episode], deadline=deadline)
    if (first['exit_status'] != 0 or capture_status(json.loads(second['stdout']), expected_episode=episode,
                                                    exit_status=second['exit_status']) != 'verified'):
        raise ValueError('native recall seed did not produce a verified fact')
    return [first, second]


def project_mcp(data):
    return data['structuredContent'] if 'structuredContent' in data else json.loads(
        next(item['text'] for item in data['content'] if item['type'] == 'text'))


@contextmanager
def trial_host(plan, case, arm, directory, work, deadline=None):
    skill, case_id = case['skill'], case.get('objective_id',case['id'])
    recall_source = case.get('source', RECALL_SOURCE)
    native = case['binding'] == 'native'
    binary = plan['binary']
    case_files = plan['skills'][skill][arm]
    references = {name: value for name, value in case_files.items() if name != 'SKILL.md'}
    controlled = Controlled(skill, case_id)
    product = None
    schema = None
    try:
        if skill == 'memory-mcp':
            product = DeadlineMCP({'product_binary': binary, 'launch_env': {
                **PRODUCT_ENV, 'HOME': str(work)}}, work, deadline)
            schema = {MCP_HOST + tool['name']: tool['inputSchema'] for tool in product.tools['tools']}

            def observed_call(name, args):
                if not native:
                    return controlled.mcp(name, args)
                raw = product.request('tools/call', {'name': name, 'arguments': args})
                delivered = project_mcp(raw)
                if raw.get('isError'):
                    return {'status': 'error', 'result': delivered, 'guidance': 'MCP tool returned an error.'}
                return delivered

            handlers = {name: (lambda args, name=name: observed_call(name.removeprefix(MCP_HOST), args))
                        for name in schema}
            if case_id == 2:
                ingested = product.tool('ingest', recall_source)
                extracted = product.tool('extract', {'episode_id': ingested['result']})
                if capture_status(extracted, expected_episode=ingested['result']) != 'verified':
                    raise ValueError('native recall seed did not produce a verified fact')
            host = SkillHost(skill, references, handlers, tool_schemas=
                             schema if plan.get('workflow_protocol') == 'progressive-completion/v1' else None)
        else:
            store = work / 'data'
            if case_id == 2:
                write_json(directory / 'native-seed.json', seed(store, binary, deadline, recall_source))

            def run(argv, stdin):
                if len(argv) in (3, 4) and argv[-1] in ('-h', '--help') and (
                        len(argv) == 3 or argv[1] == 'import' and argv[2] in ('file', 'url')):
                    return native_cli(binary, store, argv, help_only=True, deadline=deadline)
                if native and argv[1] in {'ingest', 'extract', 'assemble-context', 'explain',
                                           'resolve', 'invalidate'}:
                    return native_cli(binary, store, argv, stdin, deadline=deadline)
                return controlled.cli(argv, stdin)

            host = SkillHost(skill, references, {}, run)
        yield host, case_files['SKILL.md'], schema
    finally:
        try:
            if product:
                product.close()
        finally:
            # Preserve the public JSON-RPC receipts after the disposable store is removed.
            for filename in ('mcp.jsonl', 'mcp.stderr.log'):
                if (work / filename).exists():
                    (directory / filename).write_bytes((work / filename).read_bytes())


TRANSPORT_RULES = '''You are operating in an isolated Memory skill evaluation. Use the supplied skill below.
Reply with exactly one JSON object per turn, without Markdown, preamble or extra keys.
The next message will contain the actual response to the action. Read results before deciding what to do.
Actions:
{"action":"read_reference","path":"references/..."} reads a relative skill reference.
{"action":"finish","answer":"Your substantive final answer, with results and evidence."} finishes the task.
'''
MCP_ACTION = '''{"action":"call","name":"mcp__memory__operation","arguments":{...}} invokes a connected MCP tool.
Pass flat named arguments. Available exact names and live argument schemas are in host context.
'''
CLI_ACTION = '''{"action":"run","argv":["memory_mcp","command","--flag","value"],"stdin":"optional exact text"} runs one bound CLI command.
No shell parsing or external commands. The response contains stdout, stderr and exit_status separately.
'''


WORKFLOW_PROTOCOLS = ('legacy', 'progressive-completion/v1')
COMPLETION_RULES = '''\nFinish as soon as the requested operation and sufficient evidence are observed.\nKeep the final answer concise (at most 60 words), including observed IDs/source, status and time boundary when relevant.\nDo not restate instructions or reread already sufficient evidence. A pending/partial result is not verified completion.\nIf evidence is insufficient, state that limitation; never invent facts or a successful operation.\n'''
DISCLOSURE_ACTION = '''\nLive tool names are listed below; every listed tool remains available.\n{"action":"describe_tool","name":"mcp__memory__operation"} returns its exact live input schema.\nRead the exact schema when arguments are not known; do not invent fields. This is host metadata, not a memory operation.\n'''


def create_messages(case, skill_text, schema, *, workflow_protocol='legacy'):
    if workflow_protocol not in WORKFLOW_PROTOCOLS:
        raise ValueError('unregistered workflow protocol')
    if workflow_protocol == 'progressive-completion/v1':
        host = '\nLive MCP tool names:\n' + json.dumps(list(schema),ensure_ascii=False) if schema else ''
        actions = MCP_ACTION + DISCLOSURE_ACTION if schema else CLI_ACTION
        return [{'role':'system','content':TRANSPORT_RULES + actions + COMPLETION_RULES + host + '\nSkill:\n' + skill_text},
                {'role':'user','content':case['prompt'] + '\n\nAvailable scenario inputs:\n'
                 + json.dumps(case['fixture'],ensure_ascii=False,sort_keys=True)}]
    host = '\nLive MCP tools:\n' + json.dumps(schema, ensure_ascii=False) if schema else ''
    actions = MCP_ACTION if schema else CLI_ACTION
    return [{'role': 'system', 'content': TRANSPORT_RULES + actions + host + '\nSkill:\n' + skill_text},
            {'role': 'user', 'content': case['prompt'] + '\n\nAvailable scenario inputs:\n'
             + json.dumps(case['fixture'], ensure_ascii=False, sort_keys=True)}]


def parse_action(raw):
    action = json.loads(raw)
    if not isinstance(action, dict):
        raise ValueError('model action must be a JSON object')
    if action.get('action') == 'finish':
        answer = action.get('answer')
        if not isinstance(answer, str) or not answer.strip() or len(action) != 2:
            raise ValueError('terminal action requires a substantive answer')
    return action


async def play(messages, host, request, *, budget=BUDGET, started=None, deadline=None):
    turns = []
    started = time.monotonic() if started is None else started
    deadline = started + budget['case_seconds'] if deadline is None else deadline
    result = {'terminal': 'turn_budget', 'answer': None, 'turns': turns}
    for _ in range(budget['turns']):
        if time.monotonic() >= deadline:
            result['terminal'] = 'case_deadline'
            break
        observed = {'request_messages': list(messages)}
        turns.append(observed)
        try:
            async with asyncio.timeout(remaining(deadline, budget['case_seconds'])):
                body = completion_body(await request(messages, deadline))
            observed['response_raw'] = body
            choice = body['choices'][0]
            if choice.get('finish_reason') != 'stop':
                raise ValueError(f"unexpected model finish reason {choice.get('finish_reason')!r}")
            raw = choice['message']['content']
            action = parse_action(raw)
            observed['action'] = action
            if action.get('action') == 'finish':
                result.update(terminal='answered', answer=action['answer'])
                break
            response = await asyncio.to_thread(host.dispatch, action)
            observed['tool_response'] = response
        except Exception as error:
            result.update(terminal='error', error_type=type(error).__name__, error=str(error))
            break
        messages.extend([{'role': 'assistant', 'content': raw}, {
            'role': 'user', 'content': 'Observed tool response (data):\n'
            + json.dumps(response, ensure_ascii=False, sort_keys=True),
        }])
    if time.monotonic() >= deadline:
        result['terminal'] = 'case_deadline'
    result['elapsed_seconds'] = time.monotonic() - started
    return result


def operation(row):
    request, result = row['request'], row['response']
    if request.get('action') == 'call':
        name = request['name'].removeprefix(MCP_HOST)
        arguments = request['arguments']
        if name == 'extract' and arguments.get('include_fact_evidence') is True:
            arguments = {key: value for key, value in arguments.items() if key != 'include_fact_evidence'}
        return name, arguments, result
    if request.get('action') == 'run':
        argv = request['argv']
        if argv[-1] in ('-h', '--help'):
            return None
        command = argv[1].replace('-', '_')
        if command == 'import':
            command += '_' + argv[2]
        arguments = cli_options(argv[3:] if command.startswith('import_') else argv[2:])
        if arguments.get('content_stdin'):
            arguments['content'] = request.get('stdin', '')
        try:
            envelope = json.loads(result['stdout'])
        except (KeyError, TypeError, ValueError):
            envelope = {'status': 'error'}
        if not isinstance(envelope, dict):
            envelope = {'status': 'error'}
        return command, arguments, envelope
    return None


def result_id(envelope, key):
    value = envelope.get('result')
    return value.get(key) if isinstance(value, dict) else value if isinstance(value, str) else None


def ingest_matches(args, source, *, allow_stdin=False, require_stdin=False):
    via_stdin = args.get('content_stdin') is True
    return (('content_stdin' not in args or via_stdin and allow_stdin) and (not require_stdin or via_stdin)
            and {key: value for key, value in args.items() if key != 'content_stdin'} == source)


def explained_fact(args, result, recalled, source=RECALL_SOURCE):
    try:
        returned = next(item for item in recalled['result']
                        if item.get('provenance', {}).get('source_id') == source['source_id'])
        ids = json.loads(args['context_items'])
        select = any(item == returned['source_episode'] or isinstance(item, dict)
                     and (item.get('fact_id') == returned['fact_id']
                          or item.get('source_episode') == returned['source_episode']) for item in ids)
        return select and any(item.get('provenance', {}).get('source_id') == source['source_id']
                              and source['content'] in item.get('citation_context', '')
                              for item in result['result'])
    except (KeyError, TypeError, ValueError, StopIteration):
        return False



def fresh_explained_fact(args, result, recalled, source):
    if (result.get('status') not in {'success','partial'}
            or recalled.get('status') not in {'success','partial'}
            or not explained_fact(args,result,recalled,source)):
        return False
    returned = next(item for item in recalled['result']
                    if item.get('provenance',{}).get('source_id') == source['source_id'])
    return any(item.get('fact_id') == returned.get('fact_id')
               and canonical_id(item.get('fact_id'),'fact')
               and item.get('source_episode') == returned.get('source_episode')
               and canonical_id(item.get('source_episode'),'episode')
               and item.get('provenance',{}).get('source_id') == source['source_id']
               and source['content'] in item.get('citation_context','') for item in result['result'])

def capture_readback_matches(extracted, recalled, episode, source_id):
    items = recalled.get('result')
    if recalled.get('status') != 'success' or not isinstance(items, list):
        return False
    fact_ids = {fact['fact_id'] for fact in extracted['result']['facts']}
    for item in items:
        if not isinstance(item, dict) or item.get('source_episode') != episode:
            continue
        provenance = item.get('provenance')
        fact_id = item.get('fact_id')
        if (isinstance(provenance, dict) and provenance.get('source_id') == source_id
                and provenance.get('source_episode_id', episode) == episode
                and isinstance(fact_id, str) and fact_id in fact_ids):
            return True
    return False


def grade_observed(case, trial, trace):
    skill, case_id = case['skill'], case.get('objective_id',case['id'])
    cli = skill == 'memory-cli'
    reads = [row['request'].get('path') for row in trace if row['request'].get('action') == 'read_reference']
    allowed_reads = {'references/memory-contract.md', 'references/commands.md' if cli else 'references/mcp-tools.md'}
    observations = {
        'operation_and_exact_arguments': False, 'pending_vs_completed_oracle': None,
        'source_binding_observed': None,
        'unnecessary_reference_loading': len(reads) != len(set(reads)) or bool(set(reads) - allowed_reads),
        'reference_reads': reads,
        'process_faults': [{'operation': row['request']['argv'][1],
                            'exit_status': row['response']['exit_status']} for row in trace
                           if row['request'].get('action') == 'run'
                           and row['response'].get('exit_status') not in (0, None)],
    }
    if any(
            not isinstance(row['response'], dict) or row['response'].get('denied')
            or row['response'].get('host_error') for row in trace):
        return observations
    try:
        calls = [parsed for row in trace if (parsed := operation(row)) is not None]
    except (IndexError, KeyError, TypeError, ValueError):
        return observations
    if trial['terminal'] != 'answered' and not calls:
        return observations
    names = [name for name, _, _ in calls]
    args = [arguments for _, arguments, _ in calls]
    results = [response for _, _, response in calls]
    passed = False
    source = None
    status = None
    if case_id == 1 or case_id == 4:
        literal = case_id == 4
        expected = ['ingest'] if literal and not cli and names == ['ingest'] else ['ingest', 'extract']
        readback = cli and not literal and names == [*expected, 'assemble_context']
        if names == expected or readback:
            process_faults = observations['process_faults']
            episode = result_id(results[0], 'episode_id')
            source = (ingest_matches(args[0], LITERAL if literal else case.get('source', SOURCE), allow_stdin=cli,
                                     require_stdin=literal and cli)
                      and results[0].get('status') == 'success' and canonical_id(episode, 'episode')
                      and not any(fault['operation'] == 'ingest' for fault in process_faults))
            passed = source and (len(expected) == 1
                or args[1] == {'episode_id': episode})
            if len(expected) == 2:
                exit_status = next((fault['exit_status'] for fault in process_faults
                                    if fault['operation'] == 'extract'), 0)
                status = capture_status(results[1], expected_episode=episode, exit_status=exit_status)
            if readback:
                source = (passed and status == 'verified' and bool(args[2].get('query', '').strip())
                          and not any(fault['operation'] == 'assemble-context' for fault in process_faults)
                          and capture_readback_matches(results[1], results[2], episode, case.get('source', SOURCE)['source_id']))
                passed = source
    elif case_id == 2 and names == ['assemble_context', 'explain']:
        source = explained_fact(args[1], results[1], results[0],case.get('source',RECALL_SOURCE))
        if case.get('objective_id') is not None:
            source = (not observations['process_faults']
                      and fresh_explained_fact(args[1],results[1],results[0],case['source']))
        required = case.get('required_selectors',{})
        snapshot_matches = True
        if required:
            try:
                snapshot = results[0]['context']['snapshot']
                snapshot_matches = all(datetime.fromisoformat(snapshot[key]) == datetime.fromisoformat(value)
                                       for key,value in required.items())
            except (KeyError,TypeError,ValueError):
                snapshot_matches = False
            observations['returned_snapshot_matches'] = snapshot_matches
        passed = (bool(args[0].get('query', '').strip()) and source and snapshot_matches
                  and all(args[0].get(key)==value for key,value in required.items()))
    elif (skill, case_id) == ('memory-mcp', 3) and names == ['resolve']:
        passed = (args[0] == {'canonical_name': 'Alice Smith', 'entity_type': 'person', 'aliases': ['A. Smith']}
                  and results[0].get('status') == 'success' and bool(result_id(results[0], 'entity_id')))
    elif (skill, case_id) == ('memory-cli', 3) and names == ['reembed']:
        status = results[0].get('status')
        passed = args[0] == {'max_failures': 0} and status == 'interrupted'
    elif (skill, case_id) == ('memory-mcp', 5):
        passed = (names == ['assemble_context'] and exact_history(args[0])
                  and results[0].get('status') == 'partial')
    elif (skill, case_id) == ('memory-cli', 5):
        files = [(a, r) for n, a, r in calls if n == 'import_file']
        urls = [(a, r) for n, a, r in calls if n == 'import_url']
        extracts = [(a, r) for n, a, r in calls if n == 'extract']
        if len(files) == len(urls) == 1 and len(extracts) == 2 and len(calls) == 4:
            source = all(a == IMPORTS[kind] and r.get('status') == 'success'
                         for kind, (a, r) in zip(('file', 'url'), [files[0], urls[0]], strict=True))
            episodes = {result_id(r, 'episode_id') for _, r in (files[0], urls[0])}
            passed = (source and names.index('import_file') < names.index('import_url')
                      and all(a == {'episode_id': result_id(r, 'episode_id')}
                              and capture_status(r) == 'verified' for a, r in extracts)
                      and {a['episode_id'] for a, _ in extracts} == episodes)
    elif case_id == 6:
        explain_args = [a for n, a, _ in calls if n == 'explain']
        explain_results = [r for n, _, r in calls if n == 'explain']
        source = any(selected_long_item(a) and 'source_offset' in a
                     and any(item.get('quote_verified') is True and item.get('quote_complete') is True
                             and item.get('quote_contained_in_window') is True and item.get('quote') == QUOTE
                             and item.get('source_episode') == LONG_EPISODE
                             and item.get('provenance', {}).get('source_id') == LONG_SOURCE
                             and QUOTE in item.get('citation_context', '')
                             for item in r.get('result') or []) for a, r in zip(explain_args, explain_results))
        if cli:
            passed = (names.count('assemble_context') == 1 and set(names) <= {'assemble_context', 'explain'}
                      and exact_history(args[names.index('assemble_context')]) and source
                      and any(a.get('provenance_offset') == 20 for a in explain_args))
        else:
            passed = bool(names) and set(names) == {'explain'} and source
    elif case_id == 7:
        if cli:
            passed = names == ['extract'] and args[0] == {'episode_id': LONG_EPISODE, 'compact': True}
            if passed:
                process_faults = observations['process_faults']
                status = capture_status(results[0], expected_episode=LONG_EPISODE,
                                        exit_status=process_faults[0]['exit_status'] if process_faults else 0)
        else:
            passed = not names
            status = ['verified', 'episode-only']
    elif case_id == 8:
        episode = PENDING['result']['episode_id']
        passed = not names or (names == ['extract'] and args[0] == {'episode_id': episode}
                              and capture_status(results[0]) == 'pending')
        status = 'pending'
    observations.update(operation_and_exact_arguments=bool(passed), source_binding_observed=source,
                        pending_vs_completed_oracle=status)
    return observations


RUBRIC = {
    'operation_and_exact_arguments': 'Require actual dispatched operations and exact source/snapshot/canonical argument binding. CLI1 also accepts one post-extract assemble-context readback of a fact returned by that successful extraction, bound to the ingested episode and source ID. Textual claims never count as calls. Question-only cases can have no call. Process stdout, stderr and exit status are recorded separately.',
    'pending_vs_completed': 'Review the answer against the actual structured result and frozen expected_behavior. Partial remains pending, completed successful empty is episode-only, nonempty successful facts can be verified. Reembed interrupted remains unfinished. Do not award an outcome based on exit status alone.',
    'source_binding': 'Require the requested source and exact returned identifiers where applicable. For provenance, inspect the answers against the source/selection/quote verification in the receipt. A bare claim of verification is insufficient. Treat a wrong ID, unsupported quote or missing required citation as failure.',
    'unnecessary_reference_loading': 'Duplicate or out-of-skill reference loads are unnecessary. Both skill-specific references may be relevant to arguments, recovery, or result interpretation. A file-read attempt outside the supplied skill is denied and fails operation evidence.',
    'full_row': 'Require an observed final answer, every applicable rubric dimension, and all substantive clauses of the frozen expected_behavior. Review raw answer text for semantic clauses such as partial coverage, Unicode-scalar bounds or absence. Label raw grading disagreements explicitly; an unreviewed semantic clause is not a pass.',
}


def verify_profile(profile, binary):
    if profile == 'portable':
        return None
    if profile != 'apple-silicon':
        raise ValueError('unknown execution profile')
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        raise ValueError('native Apple Silicon evaluation process required')
    architecture = subprocess.check_output(['file', str(binary)], text=True)
    if 'Mach-O 64-bit executable arm64' not in architecture:
        raise ValueError('native Apple Silicon product binary required')
    return architecture.strip()


def verify_plan(plan):
    if plan.get('schema') != 'memory-mcp-bounded-skill-workflow/v2':
        raise ValueError('execution requires a frozen v2 workflow plan')
    if plan.get('workflow_protocol','legacy') not in WORKFLOW_PROTOCOLS:
        raise ValueError('unregistered workflow protocol')
    progressive = plan.get('workflow_protocol','legacy') == 'progressive-completion/v1'
    if (plan.get('action_protocol') != TRANSPORT_RULES + (COMPLETION_RULES if progressive else '')
            or plan.get('mcp_action') != MCP_ACTION + (DISCLOSURE_ACTION if progressive else '')
            or plan.get('cli_action') != CLI_ACTION):
        raise ValueError('protocol registration differs from selected workflow mode')
    cohort = plan.get('case_cohort','legacy')
    if cohort not in CASE_COHORTS:
        raise ValueError('case cohort registration differs')
    expected_cases = fresh_native_cases() if cohort == 'fresh-native/v1' else load_registration()[0]
    if plan['cases'] != expected_cases:
        raise ValueError('case cohort registration differs')
    sources = {'source_sha256': (Path(__file__), 'workflow implementation'),
               'attempt_observer_sha256': (OBSERVER_SOURCE, 'inference observer'),
               'mcp_binding_sha256': (MCP_BINDING_SOURCE, 'MCP binding'),
               'binary_sha256': (Path(plan['binary']), 'product binary')}
    for key, (source, label) in sources.items():
        if sha256(source.read_bytes()) != plan[key]:
            raise ValueError(f'{label} differs from frozen plan')
    loopback_endpoint(plan['endpoint'])
    verify_profile(plan['profile'], Path(plan['binary']))
    if plan['budget'] != BUDGET or tuple(plan['arms']) != ARMS:
        raise ValueError('budget or arms differ from the frozen protocol')
    if plan.get('operation_protocol') != OPERATION_PROTOCOL:
        raise ValueError('operation protocol differs from the frozen evaluator')


def freeze(path, binary, endpoint, model, *, profile='portable', workflow_protocol='legacy', case_cohort='legacy'):
    if workflow_protocol not in WORKFLOW_PROTOCOLS:
        raise ValueError('unregistered workflow protocol')
    if case_cohort not in CASE_COHORTS:
        raise ValueError('unregistered case cohort')
    loopback_endpoint(endpoint)
    binary = binary.resolve(strict=True)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError('a runnable product binary is required')
    architecture = verify_profile(profile, binary)
    cases, skills = load_registration()
    if case_cohort == 'fresh-native/v1':
        cases = fresh_native_cases()
    path.mkdir(parents=True, exist_ok=False)
    plan = {
        'schema': 'memory-mcp-bounded-skill-workflow/v2', 'cases': cases, 'skills': skills,
        'binary': str(binary), 'binary_sha256': sha256(binary.read_bytes()), 'endpoint': endpoint,
        'model': model, 'budget': BUDGET, 'platform': platform.platform(), 'profile': profile,
        'binary_architecture': architecture, 'rubric': RUBRIC, 'arms': ARMS,
        'operation_protocol': OPERATION_PROTOCOL,
        'order': 'skill then case ID, previous/current for odd cases and reversed for even cases',
        'action_protocol': TRANSPORT_RULES, 'mcp_action': MCP_ACTION, 'cli_action': CLI_ACTION,
        'source_sha256': sha256(Path(__file__).read_bytes()),
        'attempt_observer_sha256': sha256(OBSERVER_SOURCE.read_bytes()),
        'mcp_binding_sha256': sha256(MCP_BINDING_SOURCE.read_bytes()),
        'product_env': PRODUCT_ENV,
        'controlled_scope': 'Synthetic scenario responses. Import does not touch external filesystem or network. Reembed does not mutate any actual store. MCP discovery and CLI help still use the native binary.',
        'review': 'Operation/evidence observations are automatic. Review final answers against frozen expected_behavior before counting complete passes.',
    }
    if workflow_protocol != 'legacy':
        plan['workflow_protocol'] = workflow_protocol
        plan['action_protocol'] += COMPLETION_RULES
        plan['mcp_action'] += DISCLOSURE_ACTION
    if case_cohort != 'legacy':
        plan['case_cohort'] = case_cohort
    write_json(path / 'frozen-plan.json', plan)
    return plan


async def run_trial(plan, case, arm, identity, directory, recorder):
    budget = plan['budget']
    started = time.monotonic()
    deadline = started + budget['case_seconds']
    trace = []
    result = {'terminal': 'unobserved', 'answer': None, 'turns': []}
    phase = 'setup'
    try:
        with TemporaryDirectory(prefix='memory-skill-') as temporary:
            with trial_host(plan, case, arm, directory, Path(temporary), deadline) as (host, skill_text, schema):
                trace = host.trace
                phase = 'client'
                remaining(deadline, budget['case_seconds'])
                async with local_async_client(plan['endpoint'], 'EMPTY') as client:
                    async def request(messages, call_deadline):
                        seconds = remaining(call_deadline, budget['call_seconds'])
                        return await recorder.call_async(identity, 'reader', lambda: client.chat.completions.create(
                            model=plan['model'], messages=messages, temperature=budget['temperature'],
                            max_tokens=budget['max_tokens'], timeout=seconds))

                    phase = 'play'
                    result = await play(create_messages(case, skill_text, schema,
                                        workflow_protocol=plan.get('workflow_protocol','legacy')), host, request,
                                        budget=budget, started=started, deadline=deadline)
                    phase = 'cleanup'
    except Exception as error:
        result.update(terminal=f'{phase}_error', error_type=type(error).__name__, error=str(error))
    elapsed = time.monotonic() - started
    result['elapsed_seconds'] = elapsed
    if elapsed >= budget['case_seconds']:
        result['pre_deadline_terminal'] = result['terminal']
        result['terminal'] = 'case_deadline'
    try:
        grade = grade_observed(case, result, trace)
    except Exception as error:
        grade = {'operation_and_exact_arguments': False, 'error_type': type(error).__name__, 'error': str(error)}
        result.update(pre_grade_terminal=result['terminal'], terminal='grade_error')
    usage = [t['response_raw'].get('usage') for t in result['turns'] if 'response_raw' in t]
    row = {'identity': identity, 'skill': case['skill'], 'id': case['id'], 'arm': arm,
           'binding': case['binding'], 'terminal': result['terminal'], 'answer': result['answer'],
           'elapsed_seconds': elapsed, 'tokens': usage, 'grade': grade, 'semantic_review': 'pending'}
    write_json(directory / 'trial.json', {**row, 'trace': trace, 'result': result})
    return row


async def capture_trials(plan, output):
    budget = plan['budget']
    recorder = AttemptRecorder(output, max_retries=budget['retries'], timeout_seconds=budget['call_seconds'])
    rows = []
    for case in plan['cases']:
        arms = ARMS if case['id'] % 2 else ARMS[::-1]
        for arm in arms:
            identity = f"{case['skill']}-{case['id']}-{arm}"
            directory = output / identity
            directory.mkdir()
            print(identity, flush=True)
            row = await run_trial(plan, case, arm, identity, directory, recorder)
            rows.append(row)
            print(f"  {row['terminal']} operations={row['grade']['operation_and_exact_arguments']}", flush=True)
    write_json(output / 'summary.json', {'scheduled': len(plan['cases']) * len(ARMS), 'rows': rows,
                                        'operation_protocol': OPERATION_PROTOCOL})


async def capture(path):
    plan = read_json(path / 'frozen-plan.json')
    verify_plan(plan)
    output = path / 'capture'
    output.mkdir(exist_ok=False)
    await capture_trials(plan, output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='action', required=True)
    prepare = commands.add_parser('freeze')
    prepare.add_argument('output', type=Path)
    prepare.add_argument('--binary', type=Path, required=True)
    prepare.add_argument('--endpoint', required=True)
    prepare.add_argument('--model', required=True)
    prepare.add_argument('--profile', choices=('portable', 'apple-silicon'), default='portable')
    prepare.add_argument('--workflow-protocol',choices=WORKFLOW_PROTOCOLS,default='legacy')
    prepare.add_argument('--case-cohort',choices=CASE_COHORTS,default='legacy')
    execute = commands.add_parser('capture')
    execute.add_argument('output', type=Path)
    args = parser.parse_args()
    if args.action == 'freeze':
        freeze(args.output, args.binary, args.endpoint, args.model, profile=args.profile,
               workflow_protocol=args.workflow_protocol,case_cohort=args.case_cohort)
    else:
        asyncio.run(capture(args.output))


if __name__ == '__main__':
    main()
