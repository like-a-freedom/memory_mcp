DEFINE FIELD embedding ON fact TYPE option<array<float>>;
DEFINE INDEX fact_embedding_hnsw ON TABLE fact FIELDS embedding HNSW DIMENSION __FACT_EMBEDDING_DIMENSION__;
