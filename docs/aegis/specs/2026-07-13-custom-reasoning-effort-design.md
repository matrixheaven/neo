# Custom Reasoning Effort Design

## Goal

Allow each provider to define arbitrary reasoning effort strings, such as
`ultramax`, without requiring a Neo release for every new provider value.

## Core Type

Replace the closed `ReasoningEffort` enum with an open
`ReasoningEffort(String)` newtype.

- Reject empty and whitespace-only values.
- Preserve every accepted value exactly as supplied. Do not trim it or change
  its case.
- Keep named constants or constructors for Neo's common values: `minimal`,
  `low`, `medium`, `high`, `xhigh`, and `max`.
- Serialize and deserialize as a plain string so existing valid configuration
  retains the same wire format.
- Do not retain a second legacy enum or compatibility branch.

## Data Flow

Provider catalogs and user configuration parse effort values into the newtype.
Model capabilities retain all valid values, including values Neo does not know.
The TUI renders the values declared by the selected model. Runtime capability
validation still rejects a selection that the model did not declare.

OpenAI-compatible requests serialize the selected value unchanged as
`reasoning_effort`. Providers that use another reasoning mechanism, such as a
token budget or toggle, continue to reject incompatible effort selections.

Automatic reasoning selection only chooses from values declared by the model.
It must not infer the meaning or relative strength of an unknown custom value.

## User Documentation

Update the existing configuration documentation with a short example:

```toml
reasoning = { mode = "effort", effort = "ultramax" }
```

The documentation must state that effort values are provider-defined,
case-sensitive, preserved exactly, and cannot be empty. It should direct users
to their provider's model documentation for supported values.

## Validation And Errors

Configuration and catalog parsing report or discard invalid empty values at
their existing error boundary. Unknown non-empty values are valid. Error
messages must identify an invalid empty reasoning effort without exposing
unrelated configuration data.

## Verification

Focused tests cover:

- common and custom effort serialization and deserialization;
- rejection of empty and whitespace-only values;
- catalog preservation of an unknown value;
- model capability validation for a custom value;
- TUI rendering and selection of a model-declared custom value;
- exact OpenAI-compatible request serialization;
- unchanged handling for budget- and toggle-based providers.

