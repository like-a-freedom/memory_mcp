# Reusable skills

Check the target host, SDK, protocol revision, and negotiated capabilities. Verify the relevant specification and implementation before relying on an extension.

### Reusable workflows

- Put executable actions in tools, reusable decision procedures in skills, and reference data in resources. Keep authorization, validation, and safety checks in server code even if a skill describes them.
- Ship a skill when a recurring workflow requires several actions or domain judgment. Include the trigger, prerequisites, decision points, concrete tool calls, stop conditions, and what evidence to return. Keep per-call argument rules in the schema and result-specific next steps in responses.
- For the [Skills extension](https://modelcontextprotocol.io/extensions/skills/overview), advertise Resources and the skills extension; implement `skills/list`, `skills/get`, and `resources/read`. Publish a complete manifest with byte sizes and digests for stable files; use the specified dynamic form for generated content. Resolve supporting files through the originating server. Reading `SKILL.md` alone does not activate it: the host controls loading and approval.
- Test discovery from realistic user requests, loading a referenced file, manifest changes and verification failures, and the workflow on a client without Skills support. Provide a documented fallback, such as locally installed instructions, without claiming that the client loaded the remote skill.

