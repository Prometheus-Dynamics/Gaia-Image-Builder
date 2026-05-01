# `gaia-artifact-providers`

Shared contracts and catalogs for artifact providers.

## Purpose

- define how Gaia recognizes artifact-provider capabilities

## Owns

- artifact-provider traits
- artifact provider catalog types
- shared artifact-provider utility surfaces

## Artifact Domain

This domain is responsible for producing build artifacts.

Examples:

- Rust binaries and libraries
- Java artifacts
- Node bundles
- Python packages
- Go binaries

## Does Not Own

- source acquisition rules
- image strategies
- planning
- execution runtime

## Current Task List

- [x] Define an artifact provider registry type with registration and lookup.
- [x] Define artifact provider planning hooks for `gaia-plan`.
- [x] Define artifact provider output contracts shared across language providers.
- [x] Standardize provider-facing dependency and output modeling for artifact specs.
- [x] Remove the current placeholder `supports_kind()` model once registry-based selection exists.
- [x] Add provider-aware validation hooks for artifact-specific rules.
