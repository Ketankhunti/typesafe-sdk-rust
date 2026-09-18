# typesafe-sdk-rust

[![crates.io](https://img.shields.io/crates/v/typesafe-sdk.svg)](https://crates.io/crates/typesafe-sdk)
[![docs.rs](https://docs.rs/typesafe-sdk/badge.svg)](https://docs.rs/typesafe-sdk)
[![license](https://img.shields.io/crates/l/typesafe-sdk.svg)](LICENSE)

Rust SDK for [TypeSafe AI](https://typesafe.ai) — typed judgments from System One
models like **Jev**.

TypeSafe turns natural language and application state into **typed judgments and
probabilities** that code can use directly. Instead of generating text, System One
models return structured answers: probabilities, choices, and scores you can
branch on, rank with, or feed into downstream logic.

## Installation

```toml
[dependencies]
typesafe-sdk = "0.1"
```

> Requires Rust 1.88+ and uses `rustls-tls` (no OpenSSL dependency).

## Quickstart

Set your API key:

```bash
export TYPESAFE_API_KEY=apikey_...
```

Ask a question:

```rust,no_run
use typesafe_sdk::{TypeSafeClient, noul, choice, score, Question};
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

    println!("Billing: {}", response.nouls()["billing"].noul);
    println!("Tone:    {}", response.choices()["tone"].choice);
    println!("Urgency: {}", response.scores()["urgency"].score);

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
use typesafe_sdk::noul;

let q = noul("Does this message convey urgency?");
```

### Choice — pick one option

```rust
use typesafe_sdk::choice;
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
use typesafe_sdk::score;

let q = score("How frustrated is the customer?", vec![
    Some("Calm".into()),
    Some("Frustrated".into()),
    Some("Very angry".into()),
]);
```

## Configuration

```rust
use std::time::Duration;
use typesafe_sdk::{TypeSafeClient, ClientConfig, RetryPolicy};

// From environment variables (TYPESAFE_API_KEY, TYPESAFE_BASE_URL, TYPESAFE_DEFAULT_MODEL)
let client = TypeSafeClient::from_env()?;

// Or explicit
let client = TypeSafeClient::new("apikey_...")?;

// Or full control
let config = ClientConfig {
    api_key: "apikey_...".to_string(),
    base_url: "https://api.typesafe.ai".to_string(),
    default_model: "jev-latest".to_string(),
    timeout: Duration::from_secs(30),
    retry: RetryPolicy::new(3)
        .with_base_delay(Duration::from_millis(500))
        .with_max_delay(Duration::from_secs(10)),
};
let client = TypeSafeClient::from_config(config)?;
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

```rust
use typesafe_sdk::TypeSafeError;

match client.system_one(state, questions).await {
    Ok(response) => { /* ... */ }
    Err(TypeSafeError::Authentication(msg)) => { /* invalid API key */ }
    Err(TypeSafeError::RateLimit(msg)) => { /* rate limited (after retries) */ }
    Err(TypeSafeError::Validation(msg)) => { /* invalid questions */ }
    Err(e) => eprintln!("error: {e}"),
}
```

The SDK automatically retries on `429 Too Many Requests` and `529 Overloaded`
responses with exponential backoff.

## License

MIT
