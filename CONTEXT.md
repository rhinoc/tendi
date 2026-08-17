# Tendi domain context

## Skill marketplace

- `MarketplaceSource` is the normalized, installable result returned by a marketplace search. Its `source` field is the input consumed by the existing skill installer.
- A marketplace provider is an external catalog and its adapter. The adapter translates provider-specific records into `MarketplaceSource` and does not leak provider schemas into the UI.
- The marketplace aggregator searches providers, merges results by installable source, and reports partial outages as warnings.
