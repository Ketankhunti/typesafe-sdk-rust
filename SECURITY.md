# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in this SDK, please **do not** open
a public GitHub issue.

Instead, email **security@typesafe.ai** with a description of the vulnerability,
steps to reproduce, and any proof-of-concept code.

We will acknowledge receipt within 48 hours and aim to provide a fix or
mitigation within 7 days, depending on severity.

## Scope

This policy covers the `typesafeai-sdk` Rust crate and its source code in this
repository. It does not cover the TypeSafe AI API itself — report API-side
issues through your TypeSafe AI account support channels.

## Security Considerations

- **API keys** are treated as secrets. They are redacted in all `Debug` output
  and stored in a `HeaderValue` marked `sensitive` so reqwest does not log them.
- **TLS** is enforced via `rustls-tls` with no way to disable certificate
  validation. HTTP is only allowed for `localhost`/`127.0.0.1`/`::1` (for
  development and testing).
- **Redirects** are disabled by default to prevent POST body replay to a
  different host via 307/308.
- **Response bodies** are capped at 1 MiB to prevent memory exhaustion from
  a malicious or buggy server.
