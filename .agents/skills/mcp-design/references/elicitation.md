# Input and approval during work

Check the target host, SDK, protocol revision, and negotiated capabilities. Verify the relevant specification and implementation before relying on an extension.

### Input and approval during work

- Use [elicitation](https://modelcontextprotocol.io/specification/2026-07-28/client/elicitation) for missing information or approval at the point it is needed. Before a consequential write, show the exact target, action, cost or irreversible effect, and ask for a specific decision. Bind acceptance to that proposed action; revalidate before applying it if the target or price changed.
- Check the client's supported mode. Use a form for non-secret structured input and URL mode for credentials or payment secrets; never ask for those secrets in a form. Elicitation form schemas are flat objects with primitive or enum properties; arrays are limited to multi-select enums. Keep complex proposed actions on the server and identify them in the form. Handle decline, cancel, unsupported mode, and timeout without performing the pending action. Use the host's own approval controls as well when required.
- A URL-mode `accept` means consent to the interaction, not completion of payment or authorization. Verify the out-of-band result in server-side state bound to the authenticated user and proposed action before continuing. Test acceptance followed by abandonment or failure.
- For an extension task waiting on input, retain the pending operation, expose the request through `input_required`, and resume only after the corresponding `tasks/update` response. Legacy Tasks use their own elicitation flow. Test decline, cancellation, stale approval, and changed state before execution.

