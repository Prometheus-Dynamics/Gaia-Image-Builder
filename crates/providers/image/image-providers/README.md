# `gaia-image-providers`

Shared contracts and catalogs for image providers.

## Purpose

- define how Gaia recognizes image-provider capabilities

## Owns

- image-provider traits
- image provider catalog types
- shared image-provider utility surfaces

## Image Domain

This domain is responsible for turning staged system content into image outputs.

Examples:

- Buildroot
- starting-point imports
- future image backends

## Does Not Own

- source-provider logic
- artifact-provider logic
- planning
- execution runtime

## Current Task List

- [x] Define an image provider registry type with registration and lookup.
- [x] Define image provider planning hooks for `gaia-plan`.
- [x] Define shared image output contracts used by Buildroot and starting-point.
- [x] Define shared collect/archive/report result structures for image providers.
- [x] Remove the current placeholder `supports_kind()` model once registry-based selection exists.
- [x] Add provider-aware validation hooks for image-specific rules.
