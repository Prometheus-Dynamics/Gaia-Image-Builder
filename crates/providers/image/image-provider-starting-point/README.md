# `gaia-image-provider-starting-point`

Starting-point image provider implementation scaffold.

## Purpose

- implement imported-rootfs or existing-image strategies within the shared image domain

## Owns

- starting-point provider identity
- future starting-point-specific planning/execution adapters

## Important Boundary

Starting-point import is a provider strategy inside the image domain, not a separate top-level architecture concept.

## Depends On

- `gaia-image-providers`
- `gaia-spec`

## Current Task List

- [x] Add a starting-point-specific image payload type in `gaia-spec`.
- [x] Register the starting-point image provider with `gaia-image-providers`.
- [x] Define the typed operations starting-point contributes to planning.
- [x] Define the typed outputs starting-point returns to reporting and provenance.
- [x] Keep starting-point-specific behavior behind the image-provider boundary.
