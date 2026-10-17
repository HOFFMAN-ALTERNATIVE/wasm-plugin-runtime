//! CPU-bound plugin: top 10 words of the input, one `word count` pair per line.
//! Used by the benchmark because its cost scales with input size.
wit_bindgen::generate!({ path: "../../wit", world: "plugin" });

use std::collections::HashMap;

struct WordFreq;

impl Guest for WordFreq {
    fn describe() -> Info {
        Info {
            name: "wordfreq".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Top 10 words by frequency, case-insensitive".into(),
        }
    }

    fn run(input: String) -> Result<String, String> {
        let mut counts: HashMap<String, u32> = HashMap::new();
        for word in input
            .split(|c: char| !c.is_alphanumeric() && c != '\'')
            .filter(|w| !w.is_empty())
        {
            *counts.entry(word.to_lowercase()).or_default() += 1;
        }
        let mut top: Vec<(String, u32)> = counts.into_iter().collect();
        // Highest count first, ties broken alphabetically so output is stable.
        top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        top.truncate(10);
        Ok(top
            .iter()
            .map(|(w, c)| format!("{w} {c}"))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

export!(WordFreq);
