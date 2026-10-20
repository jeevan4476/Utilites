use crate::storage::ClipItem;
use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config, Matcher, Utf32Str,
};

pub fn filter_clips<'a>(items: &'a [ClipItem], query: &str) -> Vec<&'a ClipItem> {
    let q = query.trim();
    if q.is_empty() {
        return items.iter().collect();
    }

    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart);

    let mut scored: Vec<(&'a ClipItem, u32)> = Vec::new();
    let mut fallback_matches: Vec<&'a ClipItem> = Vec::new();
    let q_lower = q.to_lowercase();

    for item in items {
        let mut buf = Vec::new();
        let haystack = Utf32Str::new(&item.text, &mut buf);
        if let Some(score) = pattern.score(haystack, &mut matcher) {
            scored.push((item, score));
        } else if item.text.to_lowercase().contains(&q_lower) {
            fallback_matches.push(item);
        }
    }

    // Sort scored items: pinned first, then by score descending, then by timestamp descending
    scored.sort_by(|(a, score_a), (b, score_b)| {
        match (b.pinned, a.pinned) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => score_b.cmp(score_a).then_with(|| b.timestamp.cmp(&a.timestamp)),
        }
    });

    let mut result: Vec<&'a ClipItem> = scored.into_iter().map(|(item, _)| item).collect();
    result.extend(fallback_matches);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(id: u64, text: &str, pinned: bool) -> ClipItem {
        ClipItem {
            id,
            text: text.to_string(),
            timestamp: 1000 + id,
            pinned,
        }
    }

    #[test]
    fn test_search_empty_query() {
        let items = vec![make_item(1, "one", false), make_item(2, "two", true)];
        let res = filter_clips(&items, "");
        assert_eq!(res.len(), 2);
    }

    #[test]
    fn test_search_fuzzy_and_pinned_order() {
        let items = vec![
            make_item(1, "curl https://example.com", false),
            make_item(2, "cargo build --release", true),
            make_item(3, "docker run alpine", false),
        ];

        let res = filter_clips(&items, "cargo");
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].id, 2);

        // Pinned priority
        let items2 = vec![
            make_item(1, "sample text alpha", false),
            make_item(2, "sample text beta", true),
        ];
        let res2 = filter_clips(&items2, "sample");
        assert_eq!(res2.len(), 2);
        assert_eq!(res2[0].id, 2); // Pinned is first
        assert_eq!(res2[1].id, 1);
    }

    #[test]
    fn test_substring_fallback() {
        let items = vec![make_item(1, "some/path/to/file.txt", false)];
        let res = filter_clips(&items, "file.txt");
        assert_eq!(res.len(), 1);
    }
}
