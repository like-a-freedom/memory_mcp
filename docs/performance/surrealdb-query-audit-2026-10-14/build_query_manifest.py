#!/usr/bin/env python3
"""Build the sanitized SQL probe inventory from query-probes.json."""

from __future__ import annotations

import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parent
PROBES = ROOT / "query-probes.json"
OUTPUT = ROOT / "query-manifest.json"

# Ownership is recorded from the source audit. A missing case is an error so a
# new probe cannot silently escape the review inventory.
OWNERS = {
    "edge_cast": ("knowledge.graph", "graph expansion", "keep until task 6"),
    "edge_constructed": ("knowledge.graph", "graph expansion", "equivalent A/B probe; task 6 regression required"),
    "facts_scan": ("memory.retrieval", "lexical and temporal full-fact scan", "retain required global/relative-date coverage; task 7"),
    "facts_browse": ("memory.retrieval", "browse/facet fact scan", "legacy-schema table scan; task 7"),
    "facts_fts": ("memory.retrieval", "full-text lookup", "absent-term diagnostic only; task 8"),
    "entity_batch": ("knowledge.identity", "entity resolution", "empty result; not a representative hit workload"),
    "episode_substring": ("memory.retrieval", "episode lexical fallback", "absent-term diagnostic only; task 8"),
    "episode_scan": ("memory.retrieval", "episode context scan", "52-row projection probe; task 7"),
    "backfill_first": ("embedding.backfill", "missing-vector first-page query", "profile by pending density and dimension; task 9"),
    "backfill_cursor": ("embedding.backfill", "missing-vector cursor query", "retain cursor compatibility until task 9 proves otherwise"),
    "artifacts_upper": ("http.tasks", "artifact reconciliation upper-bound query", "empty production artifact table; task 5"),
    "tasks_due": ("http.tasks", "due-task query", "REJECT as product evidence: probe used the wrong lease field"),
    "graph_batch_cast": ("knowledge.graph", "multi-seed graph expansion", "baseline shape; task 6"),
    "graph_batch_constructed": ("knowledge.graph", "multi-seed graph expansion", "candidate shape; task 6 semantic regression required"),
    "episode_id_lookup": ("knowledge.episode", "episode identity lookup", "empty lookup diagnostic"),
    "source_fact_lookup": ("knowledge.provenance", "source-fact lookup", "empty lookup diagnostic; exact production predicate still required"),
    "alias_lookup": ("knowledge.identity", "alias membership lookup", "empty lookup; schema readiness affects plan"),
    "canonical_lookup": ("knowledge.identity", "canonical-name lookup", "empty lookup diagnostic"),
    "missing_vector_count": ("embedding.backfill", "pending-vector count", "7 samples; test across density and dimensions"),
    "claim_by_id": ("knowledge.claims", "claim identity lookup", "empty lookup diagnostic"),
    "claim_by_source": ("knowledge.claims", "claim provenance lookup", "empty lookup diagnostic"),
    "claim_relations": ("knowledge.claims", "claim relation lookup", "empty lookup diagnostic"),
    "fact_entity_links": ("knowledge.graph", "fact/entity link lookup", "empty lookup diagnostic"),
    "community_members": ("knowledge.communities", "community membership query", "legacy definition scan; upgrade then verify index plan"),
    "ann_k20_ef80": ("embedding.retrieval", "ANN retrieval", "synthetic e0 vectors, dimension 2048; not a recall result"),
    "ann_k50_ef200": ("embedding.retrieval", "ANN retrieval", "synthetic e0 vectors, dimension 2048; not a recall result"),
    "fts_full_visibility": ("memory.retrieval", "full-visibility FTS fallback", "absent-term diagnostic; not representative quality evidence"),
    "backfill_first_full": ("embedding.backfill", "missing-vector full-predicate query", "read-only production probe; duplicated baseline retained as separate capture"),
    "backfill_noindex": ("embedding.backfill", "missing-vector full-predicate query with planner hint", "diagnostic candidate only; task 9 must test dense and mixed workloads"),
}

UNMEASURED = [
    ("http.registry", "tenant discovery and pagination", "ready/due tenant index and fairness at 1/100/>100 tenants", "task 5"),
    ("http.registry", "account/auth/OIDC/API-key/quota query variants", "full request path, deny/revocation and contention", "keep security semantics; no full-pipeline sample"),
    ("provisioning", "create/upgrade/delete/suspend recovery", "disposable fresh and legacy039/HTTP44 fixtures", "tasks 2-3; production mutation prohibited"),
    ("http.app_sessions", "admission and expiry cleanup", "active/expired sessions plus concurrent admission", "task 5; no production session rows"),
    ("http.tasks", "claim/lease/heartbeat/requeue/reconciliation", "exact lease_expiry predicate and durable fault cases", "task 5; tasks_due probe is invalid"),
    ("http.subscriptions", "poll/replay/outbox repair and mutation", "sequence gaps, fanout, retry and concurrent writer", "tasks 2 and 5; production mutation prohibited"),
    ("knowledge.ingestion", "ingest/extract/resolve/invalidate transactions", "full provider path and durable output", "production mutation prohibited; real provider/corpus A/A required"),
    ("memory.retrieval", "non-empty lexical, temporal, facets and explain", "popular hits, relative dates, late rare facts, cache cold/warm", "tasks 7-8; full quality cases required"),
    ("knowledge.graph", "source siblings, hub, archival, community rebuild", "high-degree, self-loop, invalidation and page-boundary cases", "tasks 7-10"),
    ("embedding", "reembed/backfill dimension and model transitions", "17/384/768/1536/2048/3072, off, mixed-signature and concurrent writes", "tasks 3, 9 and 11"),
    ("surrealdb.session", "RPC attach/use/signin/detach counters", "per-scenario remote protocol frame counts", "Task 4 instrumentation/remote rig required"),
    ("full-pipeline", "application and database CPU/RSS/I/O/latency/quality A/A", "immutable parent/candidate, same host, provider, corpus and schema", "unmeasured; must precede numeric A/B claims"),
]


def duration_us(raw: str | None) -> float | None:
    if raw is None:
        return None
    match = re.fullmatch(r"([0-9]+(?:\.[0-9]+)?)(ns|µs|us|ms|s)", raw.strip())
    if not match:
        return None
    value = float(match.group(1))
    scale = {"ns": 0.001, "µs": 1.0, "us": 1.0, "ms": 1000.0, "s": 1_000_000.0}[match.group(2)]
    return value * scale


def nested_keys(value: object) -> set[str]:
    if isinstance(value, dict):
        return set(value) | set().union(*(nested_keys(item) for item in value.values()))
    if isinstance(value, list):
        return set().union(*(nested_keys(item) for item in value))
    return set()


def main() -> None:
    source = json.loads(PROBES.read_text(encoding="utf-8"))
    entries = []
    for probe in source["probes"]:
        case = probe["case"]
        if case not in OWNERS:
            raise SystemExit(f"query probe has no inventory owner: {case}")
        owner, caller, disposition = OWNERS[case]
        rows = probe.get("rows")
        entries.append(
            {
                "id": case,
                "owner": owner,
                "caller": caller,
                "scenario": case,
                "observed": True,
                "sampleCount": len(rows) if isinstance(rows, list) else None,
                "durationWallMedianMs": probe.get("wall_median_ms"),
                "durationSqlMedianUs": duration_us(probe.get("sql_time")),
                "plan": [
                    {"operator": step.get("operator"), "index": step.get("index")}
                    for step in probe.get("plan", [])
                ],
                "rowsPerSample": rows,
                "rowBytes": None,
                "rpcCount": None,
                "signinCount": None,
                "source": Path(probe.get("source", "unknown")).name,
                "disposition": disposition,
            }
        )

    for owner, caller, scenario, disposition in UNMEASURED:
        entries.append(
            {
                "id": f"unmeasured:{owner}:{caller}",
                "owner": owner,
                "caller": caller,
                "scenario": scenario,
                "observed": False,
                "sampleCount": None,
                "durationWallMedianMs": None,
                "durationSqlMedianUs": None,
                "plan": None,
                "rowsPerSample": None,
                "rowBytes": None,
                "rpcCount": None,
                "signinCount": None,
                "source": None,
                "disposition": disposition,
            }
        )

    manifest = {
        "schema": "memory-mcp-surreal-query-manifest/v1",
        "baselineCommit": source["baseline_commit"],
        "surrealdbVersion": source["surrealdb"],
        "productionImageDigest": "sha256:2a94eb283967c7751c88822084d92a496af8071fa99529c807ff165bde53443c",
        "captureMode": "production-read-only-probes; never a full-service benchmark",
        "captureWindowUtc": None,
        "coverage": {
            "probeCount": len(source["probes"]),
            "distinctProbeForms": len({entry["id"] for entry in entries if entry["observed"]}),
            "sampleCountsAreSqlCallCounts": False,
            "missingRowsBytesRpcSigninAreNull": True,
            "fullPipelineBaseline": "unmeasured",
            "sameHostAa": "unmeasured",
            "remoteProviderQuality": "unmeasured",
            "astInventory": {
                "reportedCallSiteCountsIncludingTests": [131, 47, 58],
                "categoryMapping": "not preserved in original audit; do not interpret as production query-template counts",
            },
        },
        "entries": entries,
        "sanitization": {
            "source": "query-probes.json contains probe labels, aggregate timings, row counts, and EXPLAIN operator/index names only",
            "rawSql": "not retained in this manifest",
            "credentials": "not retained",
            "memoryContentOrVectors": "not retained",
            "recordIds": "not retained",
        },
    }
    observed = [entry for entry in entries if entry["observed"]]
    if len(observed) != len(source["probes"]):
        raise SystemExit("manifest did not preserve every source probe")
    if any(entry["rowBytes"] is not None for entry in observed):
        raise SystemExit("unmeasured response bytes must remain null")
    forbidden_keys = {"password", "token", "sqlText", "memoryText", "vectorValues"}
    if nested_keys(manifest) & forbidden_keys:
        raise SystemExit("manifest contains a field that can expose credentials or memory content")
    OUTPUT.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {len(entries)} inventory entries ({len(source['probes'])} observed probes) to {OUTPUT}")


if __name__ == "__main__":
    main()
