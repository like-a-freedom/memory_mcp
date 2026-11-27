"""Versioned source-action evidence; final answer claims require separate review."""
import json
from datetime import datetime

PROTOCOL = 'memory-mcp-skill-source-actions/v3'
SELECTION_PROTOCOL = 'memory-mcp-skill-source-actions/v4'
PROTOCOLS = (PROTOCOL, SELECTION_PROTOCOL)


def _episode(selection):
    if isinstance(selection,dict) and any(key in selection for key in (
            'factId','sourceEpisode','citationContext','allSources','graphInsights',
            'tRef','tIngested','id','sourceType')):
        return ''
    value = selection if isinstance(selection,str) else (
        selection.get('source_episode') if isinstance(selection,dict) else None)
    return value.strip() if isinstance(value,str) else ''


def _fact(selection):
    value = selection.get('fact_id') if isinstance(selection,dict) else None
    return value if isinstance(value,str) else None


def _selected(item, selection):
    if not _episode(selection) or _episode(selection) != item.get('source_episode'):
        return False
    if isinstance(selection,str):
        return True
    return ((_fact(selection) is None or _fact(selection) == item.get('fact_id'))
            and all(not isinstance(selection.get(key),str) or not selection[key]
                    or selection[key] == item.get(key)
                    for key in ('content','quote')))


def grade_recall(case, calls, observations):
    observations.update(source_sufficient=False,canonical_fact_verified=False)
    if [name for name,_,_ in calls] != ['assemble_context','explain']:
        return observations
    (_, recall_args, recalled),(_, explain_args, explained) = calls
    if (not isinstance(recall_args.get('query'),str) or not recall_args['query'].strip()
            or observations['process_faults']
            or any(response.get('status') not in {'success','partial'} for response in (recalled,explained))):
        return observations
    source = case['source']
    try:
        selected = json.loads(explain_args['context_items'])
        if case['operation_protocol'] == SELECTION_PROTOCOL:
            if not isinstance(selected,list) or not all(_episode(entry) for entry in selected):
                return observations
            offset = explain_args.get('item_offset')
            if offset is not None:
                if type(offset) is not int or offset < 0 or offset >= len(selected):
                    return observations
                selected = [selected[offset]]
        returned = next(item for item in recalled['result']
                        if item.get('provenance',{}).get('source_id') == source['source_id']
                        and (any(_selected(item,selection) for selection in selected)
                             if case['operation_protocol'] == SELECTION_PROTOCOL else item in selected))
        episode, fact = returned['source_episode'],returned['fact_id']
        if not isinstance(episode,str) or not episode.startswith('episode:') or not episode[8:].strip():
            return observations
        fallback = (fact == 'episode_fallback:'+episode
                    and returned['provenance'].get('episode_fallback') is True)
        canonical = (isinstance(fact,str) and fact.startswith('fact:') and bool(fact[5:].strip())
                     and not returned['provenance'].get('episode_fallback'))
        if not fallback and not canonical:
            return observations
        source_only = (case['operation_protocol'] == SELECTION_PROTOCOL
                       and any(_selected(returned,selection) and (isinstance(selection,str)
                               or _fact(selection) is None) for selection in selected))
        bound_items = [item for item in explained['result']
                    if item.get('source_episode') == episode
                    and (item.get('fact_id') == fact or (source_only and item.get('fact_id') is None))
                    and item.get('provenance',{}).get('source_id') == source['source_id']
                    and item.get('provenance',{}).get('source_episode',episode) == episode
                    and item.get('citation_context_exact') is True
                    and source['content'] in item.get('citation_context','')]
        bound = bool(bound_items)
    except (KeyError,TypeError,ValueError,StopIteration):
        return observations
    required = case.get('required_selectors',{})
    snapshot_matches = True
    if required:
        try:
            snapshot = recalled['context']['snapshot']
            snapshot_matches = all(
                type(snapshot[key]) is type(value)
                and (snapshot[key] == value if key == 'known_revision' else
                     datetime.fromisoformat(snapshot[key]) == datetime.fromisoformat(value))
                for key,value in required.items())
        except (KeyError,TypeError,ValueError):
            snapshot_matches = False
        observations['returned_snapshot_matches'] = snapshot_matches
    observations.update(source_binding_observed=bool(bound),source_sufficient=bool(bound),
                        canonical_fact_verified=bool(bound and canonical
                            and any(item.get('fact_id') == fact for item in bound_items)),
                        operation_and_exact_arguments=bool(bound and snapshot_matches
                            and all(recall_args.get(key) == value for key,value in required.items())))
    return observations
