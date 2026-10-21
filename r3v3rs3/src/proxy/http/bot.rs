//! Automated bot and AI crawler detection and blocking.

/// Known AI scraper, crawler and aggressive bot signatures in User-Agent headers.
const BOT_SIGNATURES: &[&str] = &[
    "gptbot",
    "chatgpt-user",
    "oai-searchbot",
    "claudebot",
    "claude-web",
    "anthropic-ai",
    "bytespider",
    "ccbot",
    "perplexitybot",
    "cohere-ai",
    "diffbot",
    "facebookbot",
    "meta-externalagent",
    "google-extended",
    "mistralbot",
    "amazonbot",
    "omgilibot",
    "imagesiftbot",
    "scrapy",
    "timpibot",
    "velenpublicwebcrawler",
];

/// Returns true if the given User-Agent string matches known AI scrapers or aggressive bots.
pub fn is_bot_or_ai_scraper(user_agent: &str) -> bool {
    BOT_SIGNATURES
        .iter()
        .any(|&sig| contains_ignore_case(user_agent, sig))
}

/// Case-insensitive substring check without heap allocation.
fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_openai_bots() {
        assert!(is_bot_or_ai_scraper(
            "Mozilla/5.0 (compatible; GPTBot/1.2; +https://openai.com/gptbot)"
        ));
        assert!(is_bot_or_ai_scraper(
            "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; ChatGPT-User/1.0; +https://openai.com/bot)"
        ));
        assert!(is_bot_or_ai_scraper("OAI-SearchBot/1.0"));
    }

    #[test]
    fn detects_anthropic_and_cohere_bots() {
        assert!(is_bot_or_ai_scraper(
            "ClaudeBot/1.0; +https://anthropic.com"
        ));
        assert!(is_bot_or_ai_scraper(
            "Mozilla/5.0 (compatible; Claude-Web/1.0)"
        ));
        assert!(is_bot_or_ai_scraper("anthropic-ai/1.0"));
        assert!(is_bot_or_ai_scraper("cohere-ai"));
    }

    #[test]
    fn detects_crawlers_and_scrapers() {
        assert!(is_bot_or_ai_scraper(
            "Mozilla/5.0 (compatible; Bytespider; spider-feedback@bytedance.com)"
        ));
        assert!(is_bot_or_ai_scraper(
            "CCBot/2.0 (https://commoncrawl.org/faq/)"
        ));
        assert!(is_bot_or_ai_scraper(
            "PerplexityBot/1.0 (+https://perplexity.ai/perplexitybot)"
        ));
        assert!(is_bot_or_ai_scraper("Scrapy/2.11.0 (+https://scrapy.org)"));
        assert!(is_bot_or_ai_scraper("FacebookBot/1.0"));
        assert!(is_bot_or_ai_scraper("Meta-ExternalAgent/1.0"));
        assert!(is_bot_or_ai_scraper("Google-Extended"));
    }

    #[test]
    fn allows_regular_browsers() {
        assert!(!is_bot_or_ai_scraper(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36"
        ));
        assert!(!is_bot_or_ai_scraper(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15"
        ));
        assert!(!is_bot_or_ai_scraper("curl/8.7.1"));
    }
}
