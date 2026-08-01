# Provider Model Refresh Spec Brief

Status: approved

## Goal

Let a user refresh the selected provider's configured models from the current
`models.dev` catalog without leaving `/provider` or rebuilding the provider's
connection settings.

## Approved Behavior

- In `/provider`, pressing `R` or `r` on a provider row starts a background
  refresh for that provider.
- Refresh uses `https://models.dev/api.json` through Neo's existing catalog
  fetch path.
- A second refresh request while a catalog fetch is active does not replace or
  abort the active request; it reports that a refresh is already running.
- The dialog remains open while the footer shows progress.
- Success reports the provider id and refreshed model count, reloads runtime
  config, and keeps `/provider` open.
- Pressing `R` or `r` on the add row has no effect.

## Configuration Update

The config mutation is one atomic read-modify-write operation:

1. Convert the fetched catalog entry to Neo model configs before writing.
2. Preserve the selected provider's complete `ProviderConfig`, including its
   display name, protocol, base URL, inline key, and key environment variable.
3. Remove every configured model whose `provider` equals the selected provider.
4. Insert the fetched catalog's complete model set using canonical catalog
   aliases and metadata.
5. Preserve every other provider and model unchanged.

If the current default model belongs to the refreshed provider:

- when the same underlying model id still exists, select its refreshed
  canonical alias;
- when it no longer exists, select the first refreshed catalog model and reset
  runtime reasoning to Neo's automatic selection for that model;
- keep `default_provider` synchronized with the selected provider.

If the current default model belongs to another provider, neither the default
model nor runtime reasoning changes.

## Failure Safety

Fetching, provider lookup, catalog conversion, and empty-model validation all
finish before the atomic config mutation begins. A network error, task failure,
missing provider id, unsupported provider entry, empty model set, or config
write failure leaves the previous parseable config intact and reports an error.

## Ownership

- `neo-tui` provider manager owns the `R` input, hint, selected provider id, and
  refresh action.
- `neo-agent` interactive catalog handling owns the existing background fetch,
  progress, completion, status, config reload, and dialog refresh.
- `neo-agent` config mutations own the atomic model replacement and default
  model transition.
- `neo-ai` keeps its existing catalog fetch and conversion behavior unchanged.

## Non-Goals

- refreshing from a provider's `/models` endpoint;
- refreshing custom registry URLs;
- persisting provider origin metadata;
- changing provider credentials, protocol, base URL, or display name;
- merging or preserving hand-written models owned by the refreshed provider;
- adding confirmation, automatic periodic refresh, cache, retry, or a new
  network client;
- changing `/model` behavior or unrelated provider-management interactions.

## Acceptance

1. `R` and `r` on a provider row emit a refresh action for exactly that
   provider; the add row and delete confirmation do not refresh.
2. A successful refresh replaces only the selected provider's models while
   preserving its provider settings and all unrelated config.
3. A surviving default model maps to its refreshed canonical alias.
4. A removed default model switches to the first refreshed model and receives
   automatic reasoning selection.
5. Fetch, lookup, conversion, empty-catalog, and write failures preserve the
   previous config.
6. The asynchronous UI shows progress, rejects overlapping refresh requests,
   reports completion or failure, reloads runtime config, and leaves
   `/provider` usable.

