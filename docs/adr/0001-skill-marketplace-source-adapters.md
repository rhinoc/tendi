# Skill marketplace source adapters

Skill marketplace integrations use one provider adapter per source and return a shared `MarketplaceSource`; the aggregator owns concurrency, de-duplication, and partial-failure handling, while the existing installer remains the single consumer of the normalized `source` string. This keeps provider schemas out of the UI and makes a new integration a local adapter plus one registry entry, at the cost of exposing only metadata shared by all providers.
