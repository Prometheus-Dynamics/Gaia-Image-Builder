# `gaia-image-provider-buildroot`

Buildroot image provider implementation scaffold.

## Purpose

- implement the Buildroot strategy within the shared image domain

## Owns

- Buildroot provider identity
- future Buildroot-specific planning/execution adapters

## Important Boundary

Buildroot is a provider, not an architectural layer.

Gaia should model `image` as the domain and Buildroot as one implementation of that domain.

## Depends On

- `gaia-image-providers`
- `gaia-spec`

## Current Task List

- [x] Add a Buildroot-specific image payload type in `gaia-spec`.
- [x] Register the Buildroot image provider with `gaia-image-providers`.
- [x] Define the typed operations Buildroot contributes to planning.
- [x] Define the typed outputs Buildroot returns to reporting and provenance.
- [x] Keep Buildroot-specific behavior behind the image-provider boundary.
