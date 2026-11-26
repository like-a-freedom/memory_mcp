# Files and context cost

Check the target host, SDK, protocol revision, and negotiated capabilities. Verify the relevant specification and implementation before relying on an extension.

### Files and context cost

- When a workflow needs a PDF, image, or spreadsheet, consider the [experimental File Uploads extension](https://github.com/modelcontextprotocol/experimental-ext-file-uploads) to transfer bytes outside model context. Verify support in both host and server and pin the compatible extension revision; the extension can change.
- For the referenced experimental upload draft, request the upload before creating a task. On retry, verify `requestState` and its binding to tool, arguments, and authenticated principal; confirm actual bytes at storage, enforce size and type, and verify any required digest before executing or creating the task. For unsafe-to-repeat calls, consume request state atomically as specified. A lost retry response leaves the outcome unknown; do not automatically repeat unless it is known not to have run. Use an idempotency key for a new call when duplicate effects matter.
- Authorize access, set retention and cleanup, and return actionable errors for rejected or expired files. Test missing bytes, changed arguments or principal, duplicate retries, unknown outcomes, limits, isolation, and cleanup. For unsupported clients, document an existing authorized upload path or report the limitation; never assume a server can read a client-local path.
- Tool search and deferred loading are host-specific. Measure discovery success, context tokens, and extra calls with representative tasks in each target host. Keep names and descriptions searchable; do not claim a fixed token reduction without a measurement.

