# Long-running operations and Tasks

Check the target host, SDK, protocol revision, and negotiated capabilities. Verify the relevant specification and implementation before relying on an extension.

### Long-running operations

- Use [Tasks](https://modelcontextprotocol.io/extensions/tasks/overview) when work can exceed the request deadline, must survive a disconnect, or needs input mid-operation. Return the ordinary synchronous result for short work when appropriate.
- For the referenced Tasks extension revision, advertise it and check the client's per-request extension capability before returning a task. Persist the task before returning its ID; associate it with the caller and enforce authorization on polling, updates, and cancellation. Define TTL, polling interval, progress, terminal result, and error behavior.
- Resume with the same task ID after reconnect; keep terminal results stable until expiry. Cancellation is cooperative: acknowledge the request and report the actual eventual state. For writes, define idempotency across retries and task creation so a lost response cannot repeat the side effect.
- Test disconnect and resume, expiry, unauthorized access, failed and completed results, duplicate submissions, cancellation races, and extension `input_required` followed by `tasks/update`. Check a completed tool result for `isError`; `completed` alone does not mean the tool succeeded. If the client lacks Tasks, use a bounded synchronous path when possible. Under the referenced extension revision, if the operation cannot be serviced without Tasks, return JSON-RPC `-32021` with `data.requiredCapabilities.extensions["io.modelcontextprotocol/tasks"]` set to `{}` before starting work.

### Dual-era Tasks compatibility

Dual-era compatibility applies to the whole server. First apply [protocol compatibility](protocol-compatibility.md); the rules below cover Tasks specifically.

For Tasks, route by the negotiated protocol revision, advertised capability, and per-request opt-in; a client advertising an unrelated era's capability does not enable that era. Follow the [Tasks compatibility matrix](https://tasks.extensions.modelcontextprotocol.io/seps/2663-tasks-extension) and the negotiated revision's specification.

| Request | Route and observable contract |
|---|---|
| `2025-11-25`, legacy Tasks supported, task requested according to tool negotiation | Legacy task flow: `tasks/get`, `tasks/result`, `tasks/cancel`, and `tasks/list` only if supported. Preserve legacy response and elicitation semantics. |
| Extension-capable revision, client declares `io.modelcontextprotocol/tasks` per request | Referenced extension: server chooses synchronous result or task; `tasks/get` carries the final result, and input uses `tasks/update`. `tasks/result` is unavailable; the old request `task` field is not an opt-in. |
| No capability valid for the negotiated revision | Use the documented bounded synchronous path or return an explicit limitation before starting work. |

Preserve enough task metadata to route polling and cancellation correctly; store an era marker when the wire contract cannot otherwise be determined. Preserve access to eligible legacy tasks across upgrades. Test both flows end to end, mismatched capability/version combinations, missing legacy task opt-in, the extension's synchronous result, correct result retrieval, cancellation, and authorization. Do not claim support for either era based only on the SDK exposing similarly named types.

