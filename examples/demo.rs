//! TypeSafe AI Rust SDK — Live API demo.
//!
//! Run with:
//!   cargo run --example demo
//!
//! Requires the `TYPESAFE_API_KEY` environment variable to be set.

use std::collections::HashMap;

use typesafe_sdk::{choice, noul, score, TypeSafeClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Build the client from the TYPESAFE_API_KEY env var.
    let client = TypeSafeClient::from_env()?;

    // -----------------------------------------------------------------------
    // Example 1 — Noul + Choice + Score in a single system_one call
    // -----------------------------------------------------------------------
    let mut questions: HashMap<String, typesafe_sdk::Question> = HashMap::new();
    questions.insert("billing".to_string(), noul("Is this about billing?").into());
    questions.insert(
        "tone".to_string(),
        choice(
            "What is the tone of this message?",
            [("calm".to_string(), None), ("angry".to_string(), None)].into(),
        )
        .into(),
    );
    questions.insert(
        "sentiment".to_string(),
        score(
            "How positive is this message?",
            vec![
                Some("very negative".into()),
                Some("neutral".into()),
                Some("very positive".into()),
            ],
        )
        .into(),
    );

    println!("=== system_one (noul + choice + score) ===");
    let response = client
        .system_one(
            "I was charged twice for my subscription and I'm furious about it!",
            questions,
        )
        .await?;

    println!("Model:  {}", response.model);
    println!(
        "Usage:  {} input / {} output tokens",
        response.usage.input_tokens, response.usage.output_tokens
    );
    println!();

    for (key, noul_ans) in response.nouls() {
        println!("  noul[{key}] = {:.4}", noul_ans.noul);
    }
    for (key, choice_ans) in response.choices() {
        println!(
            "  choice[{key}] = {} (confidence: {:.4})",
            choice_ans.choice, choice_ans.confidence
        );
        println!("    probabilities: {:?}", choice_ans.probabilities);
    }
    for (key, score_ans) in response.scores() {
        println!(
            "  score[{key}] = {} (confidence: {:.4})",
            score_ans.score, score_ans.confidence
        );
        println!("    legend: {:?}", score_ans.legend);
        println!("    probabilities: {:?}", score_ans.probabilities);
    }

    // -----------------------------------------------------------------------
    // Example 2 — List available models
    // -----------------------------------------------------------------------
    println!("\n=== list_models ===");
    let models = client.list_models().await?;
    for m in &models.models {
        println!("  {} — {}", m.name, m.description);
        if let Some(date) = &m.release_date {
            println!("    released: {date}");
        }
    }

    Ok(())
}
