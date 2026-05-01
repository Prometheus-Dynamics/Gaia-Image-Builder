# `gaia-source-providers`

Shared contracts and catalogs for source providers.

## Purpose

- define how Gaia recognizes source-provider capabilities

## Owns

- source-provider traits
- source provider catalog types
- shared source-provider utility surfaces

## Source Domain

This domain is responsible for where material comes from.

Examples:

- git
- path
- archive
- download

## Does Not Own

- artifact build logic
- image build logic
- planning
- execution runtime

## Current Task List

- [x] Define a source provider registry type with registration and lookup.
- [x] Define a source provider planning interface for `gaia-plan`.
- [x] Define a source provider execution interface for `gaia-exec` if needed.
- [x] Add typed capability matching for git, path, archive, and download source payloads.
- [x] Remove the current placeholder `supports_kind()` model once registry-based selection exists.
- [x] Add provider-aware validation hooks for source-specific rules.
