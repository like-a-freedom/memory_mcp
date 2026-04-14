-- Add optional project tags for project-scoped retrieval and faceting.

DEFINE FIELD OVERWRITE project ON episode TYPE option<string>;
DEFINE FIELD OVERWRITE project ON fact TYPE option<string>;

DEFINE INDEX OVERWRITE episode_project ON TABLE episode COLUMNS project;
DEFINE INDEX OVERWRITE fact_project ON TABLE fact COLUMNS project;
DEFINE INDEX OVERWRITE fact_project_type ON TABLE fact COLUMNS project, fact_type;
