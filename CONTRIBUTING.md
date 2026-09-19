# Contributing to typesafe-ai-sdk

Thank you for your interest in contributing! This SDK aims to be a clean,
idiomatic Rust client for the [TypeSafe AI](https://typesafe.ai) API.

## Getting Started

1. **Fork** the repository on GitHub.
2. **Clone** your fork locally:
   ```bash
   git clone https://github.com/<your-username>/typesafe-sdk-rust.git
   cd typesafe-sdk-rust
   ```
3. **Create a branch** for your work:
   ```bash
   git checkout -b feat/my-feature
   ```

## Development

```bash
# Build
cargo build

# Run tests (unit + doc tests)
cargo test

# Run clippy — must be warning-free
cargo clippy --all-targets -- -D warnings

# Run the live API demo (requires TYPESAFE_API_KEY)
TYPESAFE_API_KEY=apikey_... cargo run --example demo
```

## Pull Request Checklist

Before submitting a PR, please make sure:

- [ ] `cargo test` passes (all unit + doc tests).
- [ ] `cargo clippy --all-targets -- -D warnings` produces zero warnings.
- [ ] `cargo fmt -- --check` passes (code is formatted).
- [ ] New public types or functions have doc comments with examples.
- [ ] If adding a new feature, a test covers the happy path and error cases.
- [ ] If changing the wire format or public API, update `README.md` and doc
      comments accordingly.

## Code Style

- Follow standard Rust formatting (`cargo fmt`).
- Use `#[must_use]` on constructors and builder methods that return `Self`.
- Prefer `?` over `match` for `Option`/`Result` propagation.
- Keep public types minimal — expose only what users need.
- Document every public item with `///` doc comments.

## Reporting Issues

When filing an issue, include:

- Rust version (`rustc --version`).
- SDK version (from `Cargo.toml`).
- A minimal code snippet that reproduces the problem.
- The error message or unexpected behavior you observed.
- Whether it happens against the live API or only in tests.

## License

By contributing, you agree that your contributions will be licensed under the
MIT License.
