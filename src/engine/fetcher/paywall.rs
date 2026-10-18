//! Paywall and subscription barrier detection via Schema.org JSON-LD and standard markers.

use scraper::{Html, Selector};
use serde_json::Value;

/// Detect whether the HTML represents a paywalled or subscription-blocked article.
#[must_use]
pub fn detect_paywall(html: &str) -> bool {
    let lower = html.to_lowercase();
    // 1. Check Schema.org JSON-LD structured data (W3C/Google standard)
    if let Ok(script_sel) = Selector::parse("script[type=\"application/ld+json\"]") {
        let fragment = Html::parse_fragment(html);
        for script in fragment.select(&script_sel) {
            let text = script.text().collect::<String>();
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text)
                && let Some(Value::Bool(false)) = map.get("isAccessibleForFree") {
                return true;
            }
        }
    }

    // 2. Fallback to standard canonical metadata markers
    if lower.contains("isaccessibleforfree\": false")
        || lower.contains("isaccessibleforfree\":false")
        || lower.contains(r#"name="paywall""#)
        || lower.contains(r#"content="paywall""#)
    {
        return true;
    }

    // 3. Fallback to barrier phrases
    lower.contains("subscribe to continue reading")
        || lower.contains("this article is for subscribers only")
        || lower.contains("you have reached your limit of free articles")
        || lower.contains("behind a paywall")
}
