# typesafeai-sdk

[![crates.io](https://img.shields.io/crates/v/typesafeai-sdk.svg)](https://crates.io/crates/typesafeai-sdk)
[![docs.rs](https://docs.rs/typesafeai-sdk/badge.svg)](https://docs.rs/typesafeai-sdk)
[![license](https://img.shields.io/crates/l/typesafeai-sdk.svg)](https://github.com/Ketankhunti/typesafe-sdk-rust/blob/main/LICENSE)

Rust SDK for [TypeSafe AI](https://typesafe.ai) — typed judgments from System One
models like **Jev**.

TypeSafe turns natural language and application state into **typed judgments and
probabilities** that code can use directly. Instead of generating text, System One
models return structured answers: probabilities, choices, and scores you can
branch on, rank with, or feed into downstream logic.

## Installation

```toml
[dependencies]
typesafeai-sdk = "0.4"
```

> Requires Rust 1.88+ and uses `rustls-tls` (no OpenSSL dependency).

## Quickstart

Set your API key:

```bash
export TYPESAFE_API_KEY=apikey_...
```

Ask a question:

```rust,no_run
use typesafeai_sdk::{TypeSafeClient, noul, choice, score, Question};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = TypeSafeClient::from_env()?;

    let mut questions: HashMap<String, Question> = HashMap::new();
    questions.insert("billing".to_string(), noul("Is this about billing?").into());
    questions.insert(
        "tone".to_string(),
        choice("What is the tone?", [
            ("calm".to_string(), None),
            ("angry".to_string(), None),
        ].into()).into(),
    );
    questions.insert(
        "urgency".to_string(),
        score("How urgent is this?", vec![
            Some("low".into()),
            Some("medium".into()),
            Some("high".into()),
        ]).into(),
    );

    let response = client
        .system_one("I was charged twice and I need a refund now!", questions)
        .await?;

    if let Some(answer) = response.noul("billing") {
        println!("Billing: {}", answer.noul);
    }
    if let Some(answer) = response.choice("tone") {
        println!("Tone:    {}", answer.choice);
    }
    if let Some(answer) = response.score("urgency") {
        println!("Urgency: {}", answer.score);
    }

    Ok(())
}
```

## Primitives

TypeSafe provides three question types. Ask several in a single request — they
run in parallel and share the same state.

| Primitive | Builder | Returns |
|-----------|---------|---------|
| **Noul** | `noul(instructions)` | `noul: f64` — probability of "yes" in `[0, 1]` |
| **Choice** | `choice(instructions, criteria)` | `choice`, `probabilities`, `confidence` |
| **Score** | `score(instructions, criteria)` | `score`, `legend`, `probabilities`, `confidence` |

### Noul — yes/no

```rust
use typesafeai_sdk::noul;

let q = noul("Does this message convey urgency?");
```

### Choice — pick one option

```rust
use typesafeai_sdk::choice;
use std::collections::HashMap;

let q = choice("Which team should handle this?", {
    let mut m = HashMap::new();
    m.insert("billing".to_string(), Some("Payments, refunds".into()));
    m.insert("technical".to_string(), Some("Bugs, outages".into()));
    m
});
```

### Score — rate on a rubric

```rust
use typesafeai_sdk::score;

let q = score("How frustrated is the customer?", vec![
    Some("Calm".into()),
    Some("Frustrated".into()),
    Some("Very angry".into()),
]);
```

## Configuration

```rust,no_run
# use std::time::Duration;
# use typesafeai_sdk::{TypeSafeClient, ClientConfig, RetryPolicy};
# fn main() -> Result<(), Box<dyn std::error::Error>> {
// From environment variables (TYPESAFE_API_KEY, TYPESAFE_BASE_URL, TYPESAFE_DEFAULT_MODEL)
let client = TypeSafeClient::from_env()?;

// Or explicit
let client = TypeSafeClient::new("apikey_...")?;

// Or full control via the builder (ClientConfig is #[non_exhaustive],
// so use ClientConfig::new() and the .with_*() methods):
let config = ClientConfig::new("apikey_...")
    .with_base_url("https://api.typesafe.ai")
    .with_default_model("jev-latest")
    .with_timeout(Duration::from_secs(30))
    .with_retry(
        RetryPolicy::new(3)
            .with_base_delay(Duration::from_millis(500))
            .with_max_delay(Duration::from_secs(10)),
    );
let client = TypeSafeClient::from_config(config)?;

// You can also supply a custom reqwest::Client (e.g. to share a connection
// pool or apply custom TLS settings):
let custom_http = reqwest::Client::builder()
    .timeout(Duration::from_secs(30))
    .build()?;
let config = ClientConfig::new("apikey_...").with_http_client(custom_http);
let client = TypeSafeClient::from_config(config)?;
# Ok(())
# }
```

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `TYPESAFE_API_KEY` | — | Required. Your API key. |
| `TYPESAFE_BASE_URL` | `https://api.typesafe.ai` | API base URL. |
| `TYPESAFE_DEFAULT_MODEL` | `jev-latest` | Default model. |

## Error handling

All SDK operations return `Result<T, TypeSafeError>`. Errors map to typed
variants:

```rust,ignore
use typesafeai_sdk::{TypeSafeError, ErrorKind};

match client.system_one(state, questions).await {
    Ok(response) => { /* ... */ }
    Err(e) => match e.kind() {
        ErrorKind::Authentication(msg) => { /* invalid API key */ }
        ErrorKind::RateLimit(msg) => { /* rate limited (after retries) */ }
        ErrorKind::Validation(msg) => { /* invalid questions */ }
        _ => eprintln!("error: {e}"),
    },
}
```

The SDK automatically retries on `429 Too Many Requests`, `529 Overloaded`,
and `500`/`502`/`503`/`504` server errors, as well as network timeouts and
connection failures, with exponential backoff and equal jitter.

### Retry safety for POST requests

`system_one` is a `POST` request. The SDK retries on transient failures
(timeouts, 5xx, 429), but a client-side timeout does **not** guarantee the
server did not process the request. If the endpoint has side effects (credit
consumption, usage recording), a retry may result in duplicate processing.

To mitigate this, errors that occur while *reading the response body* —
including timeouts and truncated bodies — are classified as non-retryable
so the SDK won't replay a POST whose body the server already received.

Future versions will support an `Idempotency-Key` header to make retries
fully safe.

## License

MIT
