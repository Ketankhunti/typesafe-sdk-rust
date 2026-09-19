# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/Ketankhunti/typesafe-sdk-rust/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.1.0

> **Note:** The crate is published to crates.io as `typesafe-ai-sdk`. The GitHub repository name remains `typesafe-sdk-rust`.
