# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.1] - 2026-09-21

### Fixed

- **README installation version was stale**: Showed `"0.3"` instead of
  `"0.4"` in the `Cargo.toml` dependency example.

## [0.4.0] - 2026-09-21

### Fixed

- **`noul()` doc was factually wrong**: Said "instructions defaults to null
  when omitted" — carried over from the JS SDK. In Rust, `instructions` is a
  required parameter. Updated to clarify it must be passed explicitly.
- **`SystemOneResponse` exposed `request_id`/`raw` as both fields and methods**:
  `resp.request_id` (field) and `resp.request_id()` (method) compiled to
  different types, creating ambiguity. Fields are now `pub(crate)` — the
  accessor methods (`request_id()`, `raw()`) are the only public API.
  Same fix applied to `ListModelsResponse`.
- **`From<reqwest::Error>` silently lost retryability**: The blanket `From`
  impl mapped all `reqwest::Error` to non-retryable `Transport`, while the
  real send path (`map_reqwest_error`) correctly classifies timeouts and
  connection errors as retryable. Removed the `From` impl so future callers
  can't accidentally bypass the correct classification.
- **`ModelCard.description` and `Usage` fields lacked `#[serde(default)]`**:
  If the API ever omitted one of these fields, every response would fail to
  parse. Added `#[serde(default)]` to `Usage::input_tokens`,
  `Usage::output_tokens`, and `ModelCard::description`.

### Changed

- **`SystemOneRequest` is no longer exported**: It was `pub` but
  unconstructible by external users (due to `#[non_exhaustive]` with no public
  constructor) and unused by any public method. Now `pub(crate)`.
- **`SystemOneOpts::with_model` accepts `Option<impl Into<String>>`**: Was
  `Option<String>`, inconsistent with `system_one_with_model` which takes
  `Option<&str>`. Now accepts any `Option<impl Into<String>>` for flexibility.
- **`examples/` is now included in the published crate**: Previously excluded
  via `Cargo.toml` `exclude` list. The demo is the onboarding path and should
  ship with the crate.
- **`Cargo.toml` adds `documentation` field**: Points to `https://docs.rs/typesafeai-sdk`.
- **`docs.rs` builds all features**: Added `[package.metadata.docs.rs]
  all-features = true` so the `blocking` module is visible on docs.rs.

### Added

- **`Question` helper methods**: `is_noul()`, `is_choice()`, `is_score()`,
  `as_noul()`, `as_choice()`, `as_score()` — mirroring the `Answer` helpers.
- **`ScoreAnswer::score_index()`**: Returns the score as a `u32` index,
  handling the `f64` → `u32` cast so callers don't hand-roll it.
- **`#[must_use]` on all constructors**: `Noul::new`, `Choice::new`,
  `Score::new`, `RetryPolicy::new`, `RetryPolicy::none`, `SystemOneOpts::new`,
  and the free `noul()`, `choice()`, `score()` functions now warn if the
  returned value is discarded.
- **`Send + Sync` static assertions for `BlockingClient`**: Compile-time
  guarantee that the blocking client remains `Send + Sync`.
- **`SECURITY.md`**: Documents how to report vulnerabilities and the SDK's
  security considerations (key redaction, TLS enforcement, redirect
  prevention, body size cap).
- **POST retry safety warning in README**: Documents that `system_one`
  retries POSTs on transient failures, which may result in duplicate
  processing if the server already processed the request.
- **`jitter_seed` was unnecessarily `pub`**: Narrowed to `pub(crate)` — it's
  an internal implementation detail, not part of the public API.
- **`truncate` doc was misleading**: Said "at most `max_chars` characters"
  but the returned string can be up to `max_chars + 3` because of the
  appended `"..."` suffix. Doc now clarifies this.
- **`ClientConfig::default()` produces an unusable client**: `api_key`
  defaults to an empty string, which causes an authentication error on the
  first request. Added a doc note pointing users to `ClientConfig::new` or
  setting `api_key` explicitly.
- **`ErrorKind::Api` didn't explain 3xx handling**: Added doc explaining
  that 3xx responses fall into this variant, and that the default HTTP
  client disables redirects to prevent POST replay on 307/308.
- **Blocking `system_one`/`system_one_with_model` didn't document the
  `Send` bound**: The `state` parameter requires `Send` because it's moved
  to a helper thread. Added doc notes explaining this.

### Changed

- **Version bumped to 0.4.0**: This release contains breaking changes —
  `From<reqwest::Error>` was removed and `SystemOneRequest` was made
  `pub(crate)`. Per Semantic Versioning, these warrant a minor version bump
  pre-1.0.

## [0.3.8] - 2026-09-20

### Fixed

- **CI security audit step fails**: `rustsec/audit-check@v2.0.0` requires a
  `token` input and `checks: write` permission. Added both to the CI workflow.
- **Some 2xx responses treated as errors**: `kind_for_status()` listed success
  codes by hand and omitted 203, 205, 208, and others. It ran before the
  `is_success()` check, so those became `Api{status}` errors. Now uses
  `status.is_success()` as the first check.
- **Blocking client docs were stale**: Module and struct docs said
  "current-thread runtime" but it's now multi-threaded with one worker. Updated
  to accurately describe the blocking behavior and recommend
  `TypeSafeClient` or `spawn_blocking` for async code.
- **Helper thread panic was swallowed**: `expect("helper thread panicked")`
  hid the original panic message. Now uses `ScopedJoinHandle::join()` and
  `std::panic::resume_unwind` to propagate the original panic payload to
  callers (including `catch_unwind`), instead of a synthetic message.

### Changed

- **`BlockingClient` methods now take `state: impl Into<Value> + Send`**: The
  `system_one` and `system_one_with_model` methods on `BlockingClient` now
  accept `impl Into<Value> + Send` for the `state` parameter, matching the
  async `TypeSafeClient` API. This is a minor breaking change from 0.3.5
  where `state` was `&str`.

### Added

- `retry_timeout_is_capped_by_remaining_budget` integration test that verifies
  a retry's per-call timeout is actually capped at the remaining budget.

## [0.3.7] - 2026-09-20

### Fixed

- **README Configuration doctest fails in CI**: The Configuration code block
  calls `TypeSafeClient::from_env()` which fails when `TYPESAFE_API_KEY` is not
  set (as in CI). Marked the code block as `no_run` so it compiles but does not
  execute.

## [0.3.6] - 2026-09-20

### Fixed

- **Budget cap no longer shortens the first attempt**: The retry budget now
  only caps timeouts on *subsequent* attempts. The first attempt always uses
  the full configured timeout, ensuring a tight budget doesn't prematurely
  abort the initial request.
- **Budget exhaustion returns the real error**: When the retry budget is
  exhausted, the SDK now returns the last real server error (e.g. 500
  `InternalServer`) instead of a synthetic `Timeout`. A minimum useful
  window check also prevents sleeping when the remaining budget is too small
  for another attempt.
- **Blocking client works inside async contexts**: Replaced the overly broad
  async-context guard with a helper-thread `block_on` strategy. The blocking
  client can now be safely constructed, used, and dropped inside a Tokio
  async context, including `spawn_blocking` threads. The runtime is shut down
  in the background on `Drop` to avoid panics.
- **Unified status classification**: All HTTP error statuses are now
  classified by a single `kind_for_status()` helper, eliminating duplicated
  status-matching logic. Non-retryable errors (401, 400, 404, etc.) that fail
  to read their body now retain their correct `ErrorKind` instead of
  degrading to `Transport`.
- **`SystemOneResponse` Debug redaction**: The `answers` field is now redacted
  in `Debug` output (it contains the same sensitive data as `raw`, which was
  already redacted).
- **CHANGELOG formatting**: Fixed an unclosed backtick in the 0.3.5 section.
- **README LICENSE link**: Fixed broken `[LICENSE]` link that failed rustdoc
  when the README was included as a doctest.

### Changed

- **`blocking` feature now requires `tokio/rt-multi-thread`**: The blocking
  client uses a single-worker multi-threaded runtime so `Handle::block_on`
  can drive the event loop from a helper thread when called inside an async
  context.
- **README is now a doctest**: Code samples in `README.md` are verified by
  `cargo test --doc` via `#![doc = include_str!("../README.md")]`.
- **CI improvements**: Cache key now hashes `Cargo.toml` (not git-ignored
  `Cargo.lock`); MSRV job sets
  `CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback`; security audit uses
  the `rustsec/audit-check` action; added default-features clippy and test
  steps.

### Tests

- Added `first_attempt_uses_full_timeout_not_budget` — verifies the first
  attempt uses the full timeout even with a tight budget.
- Added `budget_exhausted_returns_last_error_not_synthetic_timeout` — verifies
  budget exhaustion returns the real error, not a synthetic `Timeout`.
- Added 6 blocking-client tests for async-context safety (constructors,
  methods, `spawn_blocking`, and `Drop`).
- Renamed `maps_408_to_timeout_and_retries` →
  `maps_408_to_request_timeout_and_retries` (stale name).

## [0.3.5] - 2026-09-19

### Fixed

- **`ClientConfig` `#[non_exhaustive]` documentation**: The doc said fields are
  public "for construction", but external crates cannot use struct literals due
  to `#[non_exhaustive]`. Updated to clarify that fields are public for
  inspection and mutation, and external crates must use `ClientConfig::new` or
  `ClientConfig::default` plus builder methods.
- **`BlockingClient` Drop panic docs**: Removed contradictory "This client
  never panics" wording. Now accurately states that constructors and methods
  return `ErrorKind::Runtime` instead of panicking, but `Drop` may panic if the
  client is dropped inside an async runtime.
- **`ErrorKind::InternalServer` doc**: Changed from generic "5xx" to "500",
  "502", "503", or "504" to match the actual implementation.
- **Oversized retryable response bodies**: `parse_response` now classifies the
  HTTP status *before* reading the body. A 503 with a body exceeding the 1 MiB
  cap is now correctly retried as `InternalServer` instead of becoming a
  non-retryable `Transport` error.
- **HTTP 408 handling**: Added a dedicated `ErrorKind::RequestTimeout(String)`
  variant for 408 responses, instead of abusing `Timeout(Duration::ZERO)`. This
  separates server-side timeouts from client-side timeouts.
- **Test thread cleanup**: Replaced 30-second `thread::sleep` calls in test
  server threads with 500ms sleeps, eliminating unnecessarily long-lived
  background threads.

## [0.3.4] - 2026-09-19

### Fixed

- **`system_one` retry doc inaccuracy**: The doc comment said the SDK retries
  on "5xx" errors, but only `500`, `502`, `503`, `504`, and `529` are actually
  retried. Updated to list the exact status codes.
- **`with_http_client` missing redirect note**: The doc did not mention that a
  custom client controls redirect behavior. Added a note that the SDK's default
  client disables redirects to prevent POST replay via 307/308, and a custom
  client that follows redirects does not have this protection.

### Changed

- **CI security audit**: Added a `cargo audit` step to the CI workflow to
  scan dependencies for known vulnerabilities.

## [0.3.3] - 2026-09-19

### Fixed

- **`budget_exceeded()` overflow**: `elapsed + delay` could theoretically
  overflow `Duration`'s finite maximum. Changed to
  `elapsed.saturating_add(delay)` for consistency with the rest of the retry
  code, which already uses `saturating_mul`.
- **`SystemOneResponse` Debug exposes `raw`**: The derived `Debug` impl dumped
  the entire raw API response JSON. Replaced with a custom `Debug` impl that
  redacts `raw` as `<redacted>`, matching the existing redaction in
  `SystemOneOpts` and `ClientConfig`.
- **`ListModelsResponse` Debug exposes `raw`**: Same fix applied — custom
  `Debug` impl redacts the `raw` field.
- **`SystemOneRequest` Debug exposes `state`**: The derived `Debug` impl dumped
  the entire `state` (which may contain sensitive user data) and question
  instructions. Replaced with a custom `Debug` impl that redacts `state` and
  shows only the question count.
- **`jitter_seed()` doc overstates uniqueness**: Changed "each call returns a
  different value" to "randomly seeded value suitable for jitter" — random
  values can collide, so the original wording was mathematically inaccurate.

## [0.3.2] - 2026-09-19

### Fixed

- **Retry budget is now a hard cap**: Previously, each attempt used the full
  per-attempt timeout regardless of elapsed time, so 3 × 10 s attempts could
  exceed a 30 s budget. Each attempt's timeout is now capped at
  `budget - elapsed`, and the loop stops immediately when the budget is
  exhausted.
- **`extra_body` reserved keys rejected outright**: `extra_body` can no longer
  override `state`, `model`, or `questions` via last-write-wins. Any reserved
  key in `extra_body` is now rejected with a `Validation` error, regardless of
  value type. Use the dedicated methods (`with_state`, `with_model`, or the
  `questions` argument) instead.
- **Blocking client guards all methods against async context**: Previously
  only `from_config`/`from_env` checked for an active Tokio runtime. All
  methods (`system_one`, `system_one_with_model`, `system_one_with_opts`,
  `list_models`) now return `ErrorKind::Runtime` before calling `block_on`,
  preventing a runtime panic.
- **User-Agent uses crate name**: The SDK User-Agent header now uses
  `env!("CARGO_PKG_NAME")` (`typesafeai-sdk/0.3.2`) instead of a hardcoded
  `typesafe-sdk/`, matching the published crate name.
- **Broken intra-doc links fixed**: All `[TypeSafeError::Variant]` links in doc
  comments have been corrected to `[ErrorKind::Variant]` so `cargo doc` passes
  with `-D warnings`.
- **`SystemOneOpts` Debug redacts `extra_body`**: The `Debug` impl now redacts
  `extra_body` (which may contain sensitive user data) alongside `state` and
  header values.
- **Avoid deep copy in response parsing**: `serde_json::from_value(raw.clone())`
  replaced with `T::deserialize(&raw)`, avoiding a full deep copy of the
  response body.

### Changed

- **CI now uses `--all-features`**: `cargo clippy`, `cargo test`, and `cargo doc`
  all run with `--all-features` so the `blocking` feature is always exercised.
  A `cargo doc --no-deps --all-features` step with `RUSTDOCFLAGS=-D warnings`
  has been added.
- **`.editorconfig` added**: Enforces LF line endings, UTF-8 encoding, and
  indentation rules for Rust, TOML, YAML, and Markdown files.
- **Line endings normalized**: `LICENSE` and `.github/workflows/ci.yml`
  converted from CRLF to LF.

## [0.3.1] - 2026-09-18

### Fixed

- **Bug #1: Auth headers missing with custom HTTP client**: When a custom
  `reqwest::Client` was supplied via `with_http_client()`, the `Authorization`,
  `Accept`, and `x-typesafe-sdk` headers were only set as default headers on
  the auto-built client — not on the custom one. Headers are now attached
  per-request so they work with both paths.
- **Bug #2: `extra_body` could bypass validation**: A non-object `extra_body`
  was silently dropped instead of rejected. `extra_body` that overrides
  `model` with an empty string or `questions` with an empty object now
  triggers re-validation and returns an error.
- **Bug #3: Protected headers silently ignored**: `with_extra_header()` now
  rejects protected headers (`authorization`, `accept`, `x-typesafe-sdk`,
  `content-type`, `content-length`, `transfer-encoding`, `host`, `connection`,
  `cookie`, `proxy-authorization`, `proxy-authenticate`, `te`, `trailer`,
  `upgrade`) with a `Validation` error instead of silently dropping them.
- **Bug #4: `SystemOneOpts` Debug leaked sensitive data**: Manual `Debug`
  impl now redacts `state` and `extra_headers` values.
- **Bug #5: Unbounded response body and error messages**: Response bodies
  are now read with a 1 MiB cap via `read_body_capped()`. Error messages
  extracted from response bodies are truncated to 512 characters.
- **Bug #6: Redirects could replay POST body and credentials**: The
  auto-built HTTP client now uses `redirect::Policy::none()` to prevent
  following redirects that could replay the request body and `Authorization`
  header to a different host.
- **Logic #1: Dead `extra_body` field on `SystemOneRequest`**: Removed the
  unused `extra_body: Option<Value>` field that was never serialized.
- **Logic #2: `request_id` from body overwritten by `None`**: The
  `SetMetadata` trait now only sets `request_id` when the header value is
  `Some`, preserving IDs that may have been in the response body.
- **Logic #3: `BlockingClient` panicked in async context**: `from_config()`
  and `from_env()` now detect async context via `Handle::try_current()` and
  return `ErrorKind::Runtime` instead of panicking. Runtime creation errors
  also use `ErrorKind::Runtime` instead of `ErrorKind::Validation`.
- **Logic #4: Retry budget was not a hard cap**: The retry loop now checks
  elapsed time after each attempt+sleep and stops if the budget has been
  exceeded, preventing attempts that push further past the deadline.
- **Standard #1: Double JSON parse eliminated**: `parse_response()` now
  parses the body once into a `serde_json::Value`, then deserializes `T`
  from it via `from_value`, reusing the same `Value` for the `raw` field.
  The `SetRequestId`/`SetRawBody` traits were merged into a single
  `SetMetadata` trait (not exported).
- **Standard #2: Broken error-handling example in README**: The README
  pattern-matched on `TypeSafeError::Authentication(msg)` which does not
  compile (`TypeSafeError` is a struct, not an enum). Fixed to use
  `err.kind()` matching. Also fixed stale `x-request-id` →
  `x-typesafe-request-id` in the [0.2.0] changelog.
- **Standard #3: `reqwest::Error` leaked in public API**: `ErrorKind::Transport`
  now wraps `String` instead of `reqwest::Error` via `#[from]`, so `reqwest::Error`
  no longer appears in the public API surface. Note: `reqwest::Client` is still
  exposed via `ClientConfig::http_client` and `with_http_client()`, and
  `From<reqwest::Error> for TypeSafeError` still exists. A reqwest major version
  bump remains a breaking change for users who supply a custom HTTP client.
- **Docs #1: README install version and `ClientConfig` example**: Install
  version updated from `0.1` to `0.3`. The `ClientConfig` struct literal
  example (which does not compile due to `#[non_exhaustive]`) replaced with
  the builder pattern.
- **Docs #4: Stale `ClientConfig` documentation**: Configuration example now
  includes `with_http_client()`.

### Changed

- `ErrorKind::Transport` is now `Transport(String)` instead of
  `Transport(#[from] reqwest::Error)`.
- `From<reqwest::Error> for TypeSafeError` now wraps the error as a string.
- `SystemOneOpts` no longer derives `Debug`; a manual impl redacts sensitive
  fields.
- `SystemOneRequest` no longer has an `extra_body` field.

### Added

- `ErrorKind::Runtime(String)` variant for blocking client runtime errors.
- 7 additional tests (123 total: 61 unit, 44 integration, 10 blocking, 8 doc).

## [0.3.0] - 2026-09-18

### Added

- **Per-call extra headers**: `SystemOneOpts::with_extra_header()` to attach
  custom headers to a single API call. Protected headers (`Authorization`,
  `Accept`, `x-typesafe-sdk`) are rejected with a `Validation` error to
  prevent credential leakage or SDK identification removal (changed in 0.3.1
  from silently ignoring).
- **Per-call timeout override**: `SystemOneOpts::with_timeout()` to override
  the client's default timeout for a single call.
- **Per-call retry policy override**: `SystemOneOpts::with_retry()` to use a
  different `RetryPolicy` for a single call without changing the client config.
- **Custom `reqwest::Client` injection**: `ClientConfig::with_http_client()`
  to supply a pre-built `reqwest::Client`, enabling shared connection pools,
  custom TLS configuration, or custom middleware.
- 7 additional tests (116 total: 61 unit, 39 integration, 8 blocking, 8 doc).

### Changed

- **`extra_body` is now last-write-wins**: Keys in `extra_body` that collide
  with known fields (`state`, `model`, `questions`) now **override** the known
  field, matching the Python SDK behavior. Previously, known fields took
  precedence and extra_body keys were silently dropped on collision.

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
- **`request_id` on responses and errors**: Captured from the `x-typesafe-request-id`
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

[Unreleased]: https://github.com/Ketankhunti/typesafe-sdk-rust/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.4.1
[0.4.0]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.4.0
[0.3.8]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.3.8
[0.3.7]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.3.7
[0.3.6]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.3.6
[0.3.5]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.3.5
[0.3.4]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.3.4
[0.3.3]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.3.3
[0.3.2]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.3.2
[0.3.1]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.3.1
[0.3.0]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.3.0
[0.2.0]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.2.0
[0.1.0]: https://github.com/Ketankhunti/typesafe-sdk-rust/releases/tag/v0.1.0

> **Note:** The crate is published to crates.io as `typesafeai-sdk`. The GitHub repository name remains `typesafe-sdk-rust`.
