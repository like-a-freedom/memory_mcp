"""Admit a rebuilt snapshot only after complete stored-content readback."""
import argparse
import json
from pathlib import Path


def verify_snapshot(expected, observed):
    for rows in (expected['segments'], observed['episodes']):
        if not rows or len({r['source_id'] for r in rows}) != len(rows):
            raise ValueError('snapshot sources must be nonempty and unique')
    if not (len(expected['segments']) == expected['expected_episode_count']
            == observed['episode_count'] == len(observed['episodes'])):
        raise ValueError('snapshot episode count does not match the complete input')
    def inventory(rows):
        return sorted((r['source_id'], r['content_sha256'], r['content_bytes']) for r in rows)
    if inventory(expected['segments']) != inventory(observed['episodes']):
        raise ValueError('snapshot stored content does not match the complete input')
    return {'episode_count': observed['episode_count'], 'complete': True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--registration', type=Path, required=True)
    parser.add_argument('--readback', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = verify_snapshot(json.loads(args.registration.read_text()),
                             json.loads(args.readback.read_text()))
    with args.output.open('x') as output:
        output.write(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    main()
