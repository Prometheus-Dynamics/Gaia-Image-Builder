# `gaia-artifact-provider-java`

Java artifact provider implementation scaffold.

## Purpose

- implement Java-specific artifact behavior under the shared artifact domain

## Owns

- Java provider identity
- future Java-specific artifact planning/execution adapters

## Does Not Own

- core artifact traits
- spec ownership
- planning engine
- execution engine

## Depends On

- `gaia-artifact-providers`
- `gaia-spec`

## Current Task List

- [x] Add a Java-specific artifact payload type in `gaia-spec`.
- [x] Register the Java provider with `gaia-artifact-providers`.
- [x] Define how Java artifacts emit typed build/materialize operations.
- [x] Define how Java provider-specific outputs map into the shared artifact output contract.
