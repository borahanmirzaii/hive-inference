use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub struct ChunkOutput {
    pub result: String,
    pub result_hash: String,
    pub processing_ms: u64,
}

pub fn process_chunk(chunk: &str) -> ChunkOutput {
    let start = std::time::Instant::now();

    let words: Vec<&str> = chunk.split_whitespace().collect();
    let word_count = words.len();

    // Extract keywords (words > 6 chars, deduplicated)
    let keywords: Vec<String> = words
        .iter()
        .filter(|w| w.len() > 6)
        .map(|w| {
            w.to_lowercase()
                .trim_matches(|c: char| !c.is_alphabetic())
                .to_string()
        })
        .filter(|w| !w.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(10)
        .collect();

    let positive = ["good", "great", "excellent", "positive", "success", "best", "improve"];
    let negative = ["bad", "poor", "fail", "negative", "worst", "problem", "error"];

    let pos_count = words
        .iter()
        .filter(|w| positive.contains(&w.to_lowercase().as_str()))
        .count();
    let neg_count = words
        .iter()
        .filter(|w| negative.contains(&w.to_lowercase().as_str()))
        .count();

    let sentiment = if pos_count > neg_count {
        "positive"
    } else if neg_count > pos_count {
        "negative"
    } else {
        "neutral"
    };

    let result = format!(
        "words={} keywords=[{}] sentiment={}",
        word_count,
        keywords.join(","),
        sentiment
    );

    let result_hash = sha256_hex(result.as_bytes());
    let processing_ms = start.elapsed().as_millis() as u64;

    ChunkOutput {
        result,
        result_hash,
        processing_ms,
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}
