"""Snapshot admission uses observed stored bodies, not adjacent input counts."""
import unittest

from evals.longmemeval_v2.snapshot_manifest import verify_snapshot


class SnapshotManifestTests(unittest.TestCase):
    def test_matching_declared_counts_cannot_hide_unobserved_rows(self):
        row = {'source_id': 'source-a', 'content_sha256': 'aaa', 'content_bytes': 3}
        with self.assertRaisesRegex(ValueError, 'episode count'):
            verify_snapshot({'expected_episode_count': 151, 'segments': [row]},
                            {'episode_count': 151, 'episodes': [row]})

    def test_partial_store_is_rejected_even_when_input_trajectory_count_matches(self):
        expected = {'expected_episode_count': 2, 'segments': [
            {'source_id': 'source-a', 'content_sha256': 'aaa', 'content_bytes': 3},
            {'source_id': 'source-b', 'content_sha256': 'bbb', 'content_bytes': 4},
        ]}
        observed = {'episode_count': 1, 'episodes': [
            {'source_id': 'source-a', 'content_sha256': 'aaa', 'content_bytes': 3},
        ]}
        with self.assertRaisesRegex(ValueError, 'episode count'):
            verify_snapshot(expected, observed)

    def test_equal_count_cannot_hide_a_replaced_or_modified_episode(self):
        expected = {'expected_episode_count': 1, 'segments': [
            {'source_id': 'source-a', 'content_sha256': 'aaa', 'content_bytes': 3},
        ]}
        observed = {'episode_count': 1, 'episodes': [
            {'source_id': 'source-a', 'content_sha256': 'changed', 'content_bytes': 3},
        ]}
        with self.assertRaisesRegex(ValueError, 'stored content'):
            verify_snapshot(expected, observed)

    def test_duplicate_manifest_cannot_qualify_a_store_with_repeated_receipts(self):
        row = {'source_id': 'source-a', 'content_sha256': 'aaa', 'content_bytes': 3}
        with self.assertRaisesRegex(ValueError, 'unique'):
            verify_snapshot({'expected_episode_count': 2, 'segments': [row, row]},
                            {'episode_count': 2, 'episodes': [row, row]})

    def test_complete_readback_accepts_different_query_order(self):
        a = {'source_id': 'source-a', 'content_sha256': 'aaa', 'content_bytes': 3}
        b = {'source_id': 'source-b', 'content_sha256': 'bbb', 'content_bytes': 4}
        actual = verify_snapshot({'expected_episode_count': 2, 'segments': [a, b]},
                                 {'episode_count': 2, 'episodes': [b, a]})
        self.assertEqual(actual, {'episode_count': 2, 'complete': True})
