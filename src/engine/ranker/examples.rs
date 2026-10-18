use super::Ranker;


impl Ranker {
    pub fn extract_examples(text: &str) -> Vec<crate::schema::response::ExtractedExample> {
        let mut examples = Vec::new();
        let mut in_block = false;
        let mut current_lang = None;
        let mut current_snippet = Vec::new();

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("```") {
                if in_block {
                    // Ending block
                    let snippet = current_snippet.join("\n");
                    if !snippet.trim().is_empty() {
                        examples.push(crate::schema::response::ExtractedExample {
                            language: current_lang.take(),
                            snippet,
                            context: None,
                            relevance: 1.0,
                        });
                    }
                    current_snippet.clear();
                    in_block = false;
                } else {
                    // Starting block
                    in_block = true;
                    let tag = trimmed.trim_start_matches("```").trim();
                    current_lang = if !tag.is_empty() {
                        Some(tag.to_string())
                    } else {
                        None
                    };
                }
            } else if in_block {
                current_snippet.push(line);
            }
        }
        examples
    }

}
