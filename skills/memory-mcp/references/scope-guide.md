# Scope Guide

The narrowest suitable scope is a hard contract. This file is the decision aid; the rule lives in the parent skill.

## Decision tree

1. Is the content restricted to a domain (HR records, security incidents, customer PII, payment data)? → `private-domain`.
2. Is it the agent's own private note or scratchpad? → `personal`.
3. Is it shared with a working group of people (a team, a project, a department)? → `team`.
4. Is it company-wide (a policy, a public announcement, an org-wide decision)? → `org`.

When two scopes fit, pick the narrower one. Widen only when verified evidence requires it.

## Scope semantics

| Scope | Access | Typical content |
|---|---|---|
| `private-domain` | Domain ACL only | HR records, security incidents, customer PII, payment data, legal hold |
| `personal` | Owner only | Private notes, individual tasks, drafts the agent has not yet verified |
| `team` | Members of the named team | Team decisions, shared project state, internal team comms |
| `org` | All org members | Policies, public announcements, org-wide metrics, cross-team decisions |

## Scope interactions

- An `assemble_context` call returns only facts at or below the requested scope. Asking for `org` will not surface `team` facts. Asking for `team` will not surface `private-domain` facts even if the agent would otherwise have access.
- An `invalidate` call does not change scope; it changes validity time. To restrict an already-captured fact, ingest a new fact at the narrower scope and invalidate the old one with a reason that names the restriction.
- A capture that is recorded at the wrong scope is not "moved" — it is re-ingested under a new `source_id` at the correct scope, and the original is invalidated.

## Common errors

- **Storing shared work in `personal`.** A team decision recorded in `personal` is invisible to the rest of the team. Symptom: `assemble_context` returns nothing for the team's own decision.
- **Storing customer PII in `org`.** PII belongs in `private-domain`. Symptom: a later audit reveals the fact in a broader index than its access policy allows.
- **Storing drafts in `team` or `org`.** Drafts are not verified and do not belong in shared scopes. Symptom: a draft is cited as a decision in a later session.

The cure in each case is the same: pick the scope that matches the **access policy** of the content, not the audience the agent happens to be talking to right now.
