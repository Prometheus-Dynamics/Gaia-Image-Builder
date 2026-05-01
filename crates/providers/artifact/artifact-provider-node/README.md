# `gaia-artifact-provider-node`

Node artifact provider implementation scaffold.

## Purpose

- implement Node-specific artifact behavior under the shared artifact domain

## Owns

- Node provider identity
- future Node-specific artifact planning/execution adapters

## Does Not Own

- core artifact traits
- spec ownership
- planning engine
- execution engine

## Depends On

- `gaia-artifact-providers`
- `gaia-spec`

## Current Task List

- [x] Add a Node-specific artifact payload type in `gaia-spec`.
- [x] Register the Node provider with `gaia-artifact-providers`.
- [x] Define how Node artifacts emit typed build/materialize operations.
- [x] Define how Node provider-specific outputs map into the shared artifact output contract.
