from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory
from threading import Barrier
import unittest
from unittest.mock import patch

from evals.longmemeval_v2.memory_mcp_backend import MemoryMcpBackend


def completed_extraction(episode_id='episode:issued'):
    return json.dumps({'status':'success','result':{'episode_id':episode_id,'entities':[],'facts':[],'links':[]}})


class MemoryMcpBackendTests(unittest.TestCase):
    def test_pending_extraction_cannot_count_as_a_built_trajectory(self):
        responses = iter([
            '{"result":{"episode_id":"episode:issued"}}',
            '{"status":"partial","result":{"episode_id":"episode:issued","facts":[]}}',
        ])
        backend = MemoryMcpBackend({}, runner=lambda *_: next(responses))
        with self.assertRaisesRegex(RuntimeError, 'incomplete extraction'):
            backend.insert({'id': 'history', 'content': 'A complete source'})
        self.assertEqual(backend.stats['insert_count'], 0)

    def test_unobserved_extraction_status_cannot_count_as_a_built_trajectory(self):
        responses = iter(['{"result":{"episode_id":"episode:issued"}}', '{}'])
        backend = MemoryMcpBackend({}, runner=lambda *_: next(responses))
        with self.assertRaisesRegex(RuntimeError, 'incomplete extraction'):
            backend.insert({'content': 'A source'})
        self.assertEqual(backend.stats['insert_count'], 0)

    def test_success_without_usable_extract_result_cannot_count_as_built(self):
        for result in (None, {'episode_id':'episode:issued'}, {'episode_id':'episode:issued','facts':None}):
            with self.subTest(result=result):
                responses = iter(['{"result":"episode:issued"}', json.dumps({'status':'success','result':result})])
                backend = MemoryMcpBackend({}, runner=lambda *_: next(responses))
                with self.assertRaisesRegex(RuntimeError, 'incomplete extraction'):
                    backend.insert({'content': 'A source'})
                self.assertEqual(backend.stats['insert_count'], 0)

    def test_success_for_another_episode_cannot_count_as_built(self):
        responses = iter(['{"result":"episode:issued"}', '{"status":"success","result":{"episode_id":"episode:wrong","facts":[]}}'])
        backend = MemoryMcpBackend({}, runner=lambda *_: next(responses))
        with self.assertRaisesRegex(RuntimeError, 'incomplete extraction'):
            backend.insert({'content': 'A source'})
        self.assertEqual(backend.stats['insert_count'], 0)

    def test_receipt_is_started_before_dispatch_and_finalized_without_raw_content(self):
        with TemporaryDirectory() as directory:
            journal = Path(directory, 'formation-attempts.jsonl')
            extract = '{"status":"success","result":{"episode_id":"episode:issued","facts":[]}}'

            def runner(command, *args):
                if command == 'ingest':
                    self.assertTrue(journal.is_file(), 'start must be visible before ingest dispatch')
                    started, = [json.loads(line) for line in journal.read_text().splitlines()]
                    body = args[args.index('--content') + 1].encode()
                    self.assertEqual(started['body_sha256'], hashlib.sha256(body).hexdigest())
                    self.assertEqual(started['body_bytes'], len(body))
                    self.assertEqual(started['source_id'], args[args.index('--source-id') + 1])
                    return '{"result":"episode:issued"}'
                return extract

            backend = MemoryMcpBackend({'formation_attempts_path':str(journal)}, runner=runner)
            backend.insert({'content': 'Sensitive crème receipt body'})
            text = journal.read_text()
            started, finished = [json.loads(line) for line in text.splitlines()]
            self.assertEqual(started['event'], 'formation_started')
            self.assertEqual(finished['event'], 'formation_finished')
            self.assertEqual(finished['attempt_id'], started['attempt_id'])
            self.assertEqual(finished['episode_id'], 'episode:issued')
            self.assertEqual(finished['observed_status'], 'success')
            self.assertEqual(finished['fact_count'], 0)
            self.assertEqual(finished['output_sha256'], hashlib.sha256(extract.encode()).hexdigest())
            self.assertNotIn('Sensitive', text)

    def test_extract_timeout_keeps_a_failed_receipt_for_the_attempt(self):
        with TemporaryDirectory() as directory:
            journal = Path(directory, 'formation-attempts.jsonl')

            def runner(command, *_):
                if command == 'ingest':
                    return '{"result":"episode:issued"}'
                raise subprocess.TimeoutExpired(['memory_mcp', 'extract'], 1, output=b'Sensitive partial output')

            backend = MemoryMcpBackend({'formation_attempts_path':str(journal)}, runner=runner)
            with self.assertRaises(subprocess.TimeoutExpired):
                backend.insert({'content':'A source'})
            self.assertTrue(journal.is_file(), 'failed attempt needs a receipt')
            text = journal.read_text()
            started, failed = [json.loads(line) for line in text.splitlines()]
            self.assertEqual(failed['attempt_id'], started['attempt_id'])
            self.assertEqual(failed['step'], 'extract')
            self.assertEqual(failed['outcome'], 'failed')
            self.assertEqual(failed['episode_id'], 'episode:issued')
            self.assertIsNone(failed['observed_status'])
            self.assertEqual(failed['error_type'], 'TimeoutExpired')
            self.assertNotIn('Sensitive', text)
            self.assertEqual(backend.stats['insert_count'], 0)

    def test_cli_timeout_rejects_unbounded_or_invalid_configuration(self):
        for value in (0, -1, float('inf'), float('nan'), True, '10'):
            with self.subTest(value=value):
                with self.assertRaisesRegex(ValueError, 'positive finite'):
                    MemoryMcpBackend({'cli_timeout_seconds': value})

    def test_hung_cli_insert_times_out_and_reaps_the_child(self):
        children = []
        real_run, real_popen = subprocess.run, subprocess.Popen

        def start(*args, **kwargs):
            child = real_popen(*args, **kwargs)
            children.append(child)
            return child

        def invoke(_argv, **kwargs):
            kwargs.setdefault('timeout', 1.0)  # Bound the regression even before the fix.
            return real_run([sys.executable, '-c',
                             'import threading; threading.Event().wait()'], **kwargs)

        with patch('subprocess.Popen', side_effect=start), patch('subprocess.run', side_effect=invoke):
            with self.assertRaises(subprocess.TimeoutExpired) as failure:
                MemoryMcpBackend({'db_path': '/unused', 'cli_timeout_seconds': 0.2}).insert({'content': 'synthetic'})
        self.assertEqual(failure.exception.timeout, 0.2)
        self.assertEqual(len(children), 1)
        self.assertIsNotNone(children[0].poll())

    def test_backend_exposes_upstream_memory_config(self):
        backend = MemoryMcpBackend({"db_path": "/tmp/test-memory-mcp"}, runner=lambda *_: "{}")
        self.assertEqual(backend.memory_type, "memory_mcp")
        self.assertEqual(backend.memory_params, {"db_path": "/tmp/test-memory-mcp"})

    def test_insert_uses_server_episode_id_and_query_returns_text_items(self):
        calls = []
        responses = iter([
            json.dumps({"result": {"episode_id": "episode:issued"}}),
            completed_extraction(),
            json.dumps({"result": {"items": [{"content": "tea"}]}}),
        ])

        def runner(command, *args):
            calls.append((command, args))
            return next(responses)

        backend = MemoryMcpBackend({"db_path": "/tmp/test-memory-mcp"}, runner=runner)
        backend.insert({"content": "I prefer tea"})
        self.assertEqual(backend.query("What do I prefer?"), [{"type": "text", "value": "tea"}])
        self.assertEqual(calls[1], ("extract", ("--episode-id", "episode:issued")))
        self.assertNotIn("--scope", calls[0][1])

    def test_trajectory_source_identity_survives_restart_and_reordering(self):
        identities = []

        def runner(command, *args):
            if command == "ingest":
                identities.append((
                    args[args.index("--source-id") + 1], args[args.index("--t-ref") + 1],
                ))
                return json.dumps({"result": "episode:issued"})
            return completed_extraction()

        first = MemoryMcpBackend({}, runner=runner)
        first.insert({"content": "separate history"})
        first.insert({"content": "tea", "role": "user"})
        restarted = MemoryMcpBackend({}, runner=runner)
        restarted.insert({"role": "user", "content": "tea"})

        self.assertNotEqual(identities[0], identities[1])
        self.assertEqual(identities[1], identities[2])
        self.assertEqual(identities[2][1], "2000-01-01T00:00:00Z")

    def test_segmented_method_preserves_complete_ordered_trajectory_under_anno_limit(self):
        bodies = []
        identities = []

        def runner(command, *args):
            if command == "ingest":
                bodies.append(args[args.index("--content") + 1])
                identities.append(args[args.index("--source-id") + 1])
                return '{"result":"episode:issued"}'
            return completed_extraction()

        trajectory = {"id": "demo", "states": [
            {"state_index": 0, "text": "я" * 550_000},
            {"state_index": 1, "text": "the last state"},
        ]}
        backend = MemoryMcpBackend({"formation_method": "ordered-segments-v1"}, runner=runner)
        backend.insert(trajectory)

        self.assertGreater(len(bodies), 1)
        self.assertTrue(all(len(body.encode("utf-8")) <= 1_048_576 for body in bodies))
        self.assertEqual(json.loads("".join(bodies)), trajectory)
        self.assertEqual(len(set(identities)), len(bodies))
        self.assertEqual(backend.stats["insert_count"], 1)

    def test_segmented_method_keeps_adjacent_states_intact_when_each_fits(self):
        bodies = []

        def runner(command, *args):
            if command == "ingest":
                bodies.append(args[args.index("--content") + 1])
                return '{"result":"episode:issued"}'
            return completed_extraction()

        trajectory = {"states": [
            {"state_index": 0, "text": "a" * 600_000},
            {"state_index": 1, "text": "b" * 600_000},
        ]}
        MemoryMcpBackend({"formation_method": "ordered-segments-v1"}, runner=runner).insert(trajectory)

        self.assertEqual(len(bodies), 2)
        self.assertEqual(bodies[0].find('"state_index": 1'), -1)
        self.assertTrue(bodies[1].startswith('{"state_index": 1, '))
        self.assertEqual(json.loads("".join(bodies)), trajectory)

    def test_segmented_retry_reuses_source_ids_after_a_failed_extract(self):
        attempts = []
        fail = True

        def runner(command, *args):
            if command == "ingest":
                source_id = args[args.index("--source-id") + 1]
                attempts.append(source_id)
                return json.dumps({"result": source_id})
            if fail and len(attempts) == 2:
                raise RuntimeError("extract interrupted")
            return completed_extraction(args[-1])

        params = {"formation_method": "ordered-segments-v1"}
        trajectory = {"states": [{"text": "x" * 600_000}, {"text": "y" * 600_000}]}
        first = MemoryMcpBackend(params, runner=runner)
        with self.assertRaisesRegex(RuntimeError, "extract interrupted"):
            first.insert(trajectory)
        self.assertEqual(first.stats["insert_count"], 0)

        fail = False
        restarted = MemoryMcpBackend(params, runner=runner)
        restarted.insert(trajectory)
        self.assertEqual(len(attempts), 4)
        self.assertEqual(attempts[:2], attempts[2:])
        self.assertEqual(restarted.stats["insert_count"], 1)

    def test_local_cli_uses_dedicated_store_without_inheriting_provider_or_remote_db(self):
        parent = {"SURREALDB_URL": "wss://example.invalid", "EMBEDDINGS_PROVIDER": "openai"}
        replies = [
            subprocess.CompletedProcess([], 0, b'{"result":"episode:issued"}'),
            subprocess.CompletedProcess([], 0, completed_extraction().encode()),
        ]
        with patch.dict(os.environ, parent, clear=True), patch("subprocess.run", side_effect=replies) as run:
            MemoryMcpBackend({"db_path": "/tmp/test-memory-mcp"}).insert({"content": "чай"})

        argv = run.call_args_list[0].args[0]
        options = run.call_args_list[0].kwargs
        self.assertIn("--content-stdin", argv)
        self.assertNotIn("--content", argv)
        self.assertEqual(json.loads(options["input"]), {"content": "чай"})
        self.assertEqual(options["env"]["SURREALDB_DATA_DIR"], "/tmp/test-memory-mcp")
        self.assertNotIn("SURREALDB_URL", options["env"])
        self.assertNotIn("EMBEDDINGS_PROVIDER", options["env"])
        self.assertEqual(options['timeout'], 1800.0)

    def test_query_statistics_separate_copy_and_process_time(self):
        clock = [0.0]
        import shutil
        real_copy = shutil.copytree

        def copy(*args, **kwargs):
            result = real_copy(*args, **kwargs)
            clock[0] += 2.0
            return result

        def read(*args, **kwargs):
            clock[0] += 3.0
            return subprocess.CompletedProcess([], 0, b'{"result":{"items":[]}}')

        with TemporaryDirectory() as directory, patch("shutil.copytree", side_effect=copy), patch("subprocess.run", side_effect=read), patch("time.monotonic", side_effect=lambda: clock[0]):
            Path(directory, "counter").write_text("0")
            backend = MemoryMcpBackend({"db_path": directory})
            self.assertEqual(backend.query("synthetic"), [])
            self.assertEqual(backend.stats["query_copy_time_secs"], 2.0)
            self.assertEqual(backend.stats["query_cli_time_secs"], 3.0)
            self.assertEqual(backend.stats["query_time_secs"], 5.0)
            self.assertEqual(Path(directory, "counter").read_text(), "0")

    def test_each_query_starts_from_the_same_store_in_either_order(self):
        used_stores = set()

        def read(*args, env, **kwargs):
            query = args[0][-1]
            store = Path(env["SURREALDB_DATA_DIR"])
            used_stores.add(store)
            counter = store / "access-count"
            count = int(counter.read_text())
            counter.write_text(str(count + 1))
            reply = {"result": {"items": [{"content": f"{query}:{count}"}]}}
            return subprocess.CompletedProcess([], 0, json.dumps(reply).encode())

        with TemporaryDirectory() as directory, patch("subprocess.run", side_effect=read):
            counter = Path(directory) / "access-count"
            counter.write_text("0")
            backend = MemoryMcpBackend({"db_path": directory})
            forward = {query: backend.query(query) for query in ("tea", "coffee")}
            backward = {query: backend.query(query) for query in ("coffee", "tea")}

            self.assertEqual(forward, backward)
            self.assertEqual(forward["tea"], [{"type": "text", "value": "tea:0"}])
            self.assertEqual(counter.read_text(), "0")
            self.assertEqual(len(used_stores), 4)
            self.assertFalse(any(store.exists() for store in used_stores))

    def test_isolated_cli_reads_can_overlap(self):
        both_reading = Barrier(2, timeout=3)

        def read(argv, *, env, **kwargs):
            store = Path(env["SURREALDB_DATA_DIR"])
            marker = store / "marker"
            marker.write_text(argv[-1])
            both_reading.wait()
            reply = {"result": {"items": [{"content": marker.read_text()}]}}
            return subprocess.CompletedProcess([], 0, json.dumps(reply).encode())

        with TemporaryDirectory() as directory, patch("subprocess.run", side_effect=read):
            marker = Path(directory) / "marker"
            marker.write_text("original")
            backend = MemoryMcpBackend({"db_path": directory})
            with ThreadPoolExecutor(max_workers=2) as executor:
                tea = executor.submit(backend.query, "tea")
                coffee = executor.submit(backend.query, "coffee")
                self.assertEqual(tea.result(), [{"type": "text", "value": "tea"}])
                self.assertEqual(coffee.result(), [{"type": "text", "value": "coffee"}])
            self.assertEqual(marker.read_text(), "original")
            self.assertEqual(backend.stats["query_count"], 2)

    def test_blank_context_is_a_failed_read(self):
        backend = MemoryMcpBackend({}, runner=lambda *_: '{"result":{"items":[{"content":"  "}]}}')

        with self.assertRaisesRegex(RuntimeError, "nonempty text"):
            backend.query("What do I prefer?")

    def test_successful_no_hit_returns_an_empty_context(self):
        backend = MemoryMcpBackend({}, runner=lambda *_: '{"result":{"items":[]}}')

        self.assertEqual(backend.query("What do I prefer?"), [])

    def test_image_queries_are_explicitly_unsupported(self):
        backend = MemoryMcpBackend({}, runner=lambda *_: "{}")
        with self.assertRaises(NotImplementedError):
            backend.query("describe image", query_image="image-bytes")

    def test_insert_accepts_public_cli_string_result(self):
        calls = []
        responses = iter([
            json.dumps({"status": "success", "result": "episode:issued"}),
            completed_extraction(),
        ])

        def runner(command, *args):
            calls.append((command, args))
            return next(responses)

        MemoryMcpBackend({}, runner=runner).insert({"content": "tea"})
        self.assertEqual(calls[1], ("extract", ("--episode-id", "episode:issued")))


if __name__ == "__main__":
    unittest.main()
