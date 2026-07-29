-- Add embedding maintenance metadata and persisted rebuild control-plane state.

DEFINE FIELD embedding_provider ON fact TYPE option<string>;
DEFINE FIELD embedding_model ON fact TYPE option<string>;
DEFINE FIELD embedding_dimension ON fact TYPE option<int>;
DEFINE FIELD embedding_signature ON fact TYPE option<string>;
DEFINE FIELD embedding_updated_at ON fact TYPE option<datetime>;

DEFINE TABLE embedding_state SCHEMAFULL;
DEFINE FIELD status ON embedding_state TYPE string;
DEFINE FIELD active_signature ON embedding_state TYPE option<string>;
DEFINE FIELD provider ON embedding_state TYPE option<string>;
DEFINE FIELD model ON embedding_state TYPE option<string>;
DEFINE FIELD dimension ON embedding_state TYPE option<int>;
DEFINE FIELD last_job_id ON embedding_state TYPE option<string>;
DEFINE FIELD updated_at ON embedding_state TYPE datetime;

DEFINE TABLE embedding_job SCHEMAFULL;
DEFINE FIELD job_id ON embedding_job TYPE string;
DEFINE FIELD status ON embedding_job TYPE string;
DEFINE FIELD target_signature ON embedding_job TYPE string;
DEFINE FIELD provider ON embedding_job TYPE string;
DEFINE FIELD model ON embedding_job TYPE option<string>;
DEFINE FIELD dimension ON embedding_job TYPE int;
DEFINE FIELD namespaces ON embedding_job TYPE array;
DEFINE FIELD requested_at ON embedding_job TYPE datetime;
DEFINE FIELD total_facts ON embedding_job TYPE int;
DEFINE FIELD processed_facts ON embedding_job TYPE int;
DEFINE FIELD succeeded_facts ON embedding_job TYPE int;
DEFINE FIELD failed_facts ON embedding_job TYPE int;
DEFINE FIELD facts_per_second ON embedding_job TYPE option<float>;
DEFINE FIELD eta_seconds ON embedding_job TYPE option<int>;
DEFINE FIELD current_namespace ON embedding_job TYPE option<string>;
DEFINE FIELD namespace_progress ON embedding_job TYPE object;
DEFINE FIELD last_error ON embedding_job TYPE option<string>;
DEFINE FIELD started_at ON embedding_job TYPE option<datetime>;
DEFINE FIELD updated_at ON embedding_job TYPE option<datetime>;
DEFINE FIELD finished_at ON embedding_job TYPE option<datetime>;
