# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-09-18

### Added

- **Blocking client**: `BlockingClient` behind the `blocking` feature flag for
  synchronous usage without an async runtime.
- **`extra_body` support**: `SystemOneOpts` builder with `with_model()`,
  `with_state()`, and `with_extra_body()` for per-call overrides and arbitrary
  JSON fields merged into the request body.
- **`system_one_with_opts()`**: New method on `TypeSafeClient` and
  `BlockingClient` accepting `SystemOneOpts` for full per-call configuration.
- **Raw body on responses**: `raw()` accessor on `SystemOneResponse` and
  `ListModelsResponse` returning the raw JSON `serde_json::Value`.
- **`field_path` on validation errors**: `ErrorKind::ResponseValidation` now
  carries an optional `field_path` pinpointing the failing field.
- **`request_id` on responses and errors**: Captured from the `x-request-id`
  response header and injected into both `TypeSafeError` and response types.
- **Retry budget**: `RetryPolicy::with_budget()` to cap total retry time.
- **`retry-after-ms` header**: Honored alongside the standard `Retry-After`
  header for rate-limit backoff.
- **`ClientConfig` builder**: `new()`, `with_api_key()`, `with_base_url()`,
  `with_default_model()`, `with_timeout()`, `with_retry()` methods.
- 90 additional tests (109 total: 61 unit, 32 integration, 8 blocking, 8 doc).

### Changed

- **`#[non_exhaustive]`** added to all public structs and enums to allow
  future field additions without breaking changes.
- `ErrorKind::ResponseValidation` is now a struct variant with `message` and
  `field_path` fields.
- `TypeSafeError` is now a struct wrapping `ErrorKind` with a `request_id`
  field, replacing the flat enum.

## [0.1.0] - 2026-09-18

### Added

- Initial release of the TypeSafe AI Rust SDK.
- `TypeSafeClient` with `system_one()`, `system_one_with_model()`, and
  `list_models()` methods.
- Three question primitives: `noul()`, `choice()`, `score()` builder functions.
- Typed error hierarchy (`TypeSafeError`) with 13 variants covering
  authentication, rate limits, validation, transport, and response errors.
- `RetryPolicy` with exponential backoff (default: 2 retries, 500ms base
  delay, 10s max delay).
- `ClientConfig` for full control over API key, base URL, model, timeout,
  and retry policy.
- Environment variable support: `TYPESAFE_API_KEY`, `TYPESAFE_BASE_URL`,
  `TYPESAFE_DEFAULT_MODEL`.
- Live API demo (`examples/demo.rs`).
- Unit tests (19) and doc tests (5) — all passing.
- README, LICENSE, and CONTRIBUTING guide.

[Unreleased]: https://github.com/Ketankhunti/typesafe-sdk-rust/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.2.0
[0.1.0]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.1.0

> **Note:** The crate is published to crates.io as `typesafe-ai-sdk`. The GitHub repository name remains `typesafe-sdk-rust`.
