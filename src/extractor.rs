//! Statistical key-term extraction — RAGFlow `KeyTermExtractor` semantics.
//!
//! RAGFlow's `auto_keywords` parser-config option runs keyword extraction
//! over every chunk after chunking and stores the result in the chunk's
//! `important_kwd` field, which hybrid retrieval then BM25-scores with an
//! elevated weight (see `search.rs`: `5.0 * keyword_scores[i]`).
//!
//! This module implements the offline, dependency-free variant of that
//! extractor: candidate terms are ranked by
//!
//!   1. **词频 (term frequency)** — how often the term occurs in the chunk;
//!   2. **位置 (position)** — terms appearing early in the text get a boost,
//!      mirroring the intuition that document-openings carry the topic;
//!   3. **词性 proxy (part-of-speech statistics)** — without an external
//!      POS tagger, CJK terms and capitalized alpha terms are treated as
//!      noun-like (jieba's keyword path keeps nouns/proper nouns), while
//!      generic lowercase words are discounted;
//!   4. **length / phrasal quality** — longer terms and adjacent bigrams
//!      (phrases) outrank single characters and stop words.
//!
//! No external service is invoked; everything is pure string statistics.

use crate::Chunk;
use std::collections::HashMap;

/// Default number of key terms per chunk (RAGFlow `auto_keywords` topn).
pub const DEFAULT_TOP_N: usize = 10;

/// Stop words shared with `nlp.rs`; extended with generic English function
/// words that never make useful keywords.
const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "are", "was", "were", "been", "have", "has",
    "had", "not", "but", "from", "they", "will", "would", "could", "should", "can", "may", "might",
    "shall", "its", "his", "her", "our", "your", "their", "them", "a", "an", "of", "to", "in",
    "on", "at", "by", "or", "as", "is", "it", "be", "do", "does", "did", "which", "what", "who",
    "whom", "when", "where", "why", "how", "into", "over", "under", "about", "than", "then",
    "also", "very", "just", "more", "most", "some", "such", "only", "own", "same", "so", "too",
    "up", "out", "off", "if", "because", "while", "during", "after", "before", "between", "among",
    "against", "within", "without", "through", "per", "via", "etc", "e.g", "i.e",
];

fn is_cjk(ch: char) -> bool {
    matches!(
        ch,
        '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{3040}'..='\u{30FF}'
            | '\u{AC00}'..='\u{D7AF}'
    )
}

/// 常见 CJK 虚词/助词首字：以其开头的片段（如「的重要」「是人工」）
/// 几乎不可能是关键词，跳过以减少噪音。
fn is_cjk_function_char(ch: char) -> bool {
    matches!(
        ch,
        '的' | '是'
            | '也'
            | '了'
            | '在'
            | '和'
            | '与'
            | '及'
            | '或'
            | '而'
            | '之'
            | '于'
            | '为'
            | '从'
            | '对'
            | '把'
            | '被'
            | '就'
            | '都'
            | '还'
            | '又'
            | '再'
            | '但'
            | '并'
            | '且'
            | '若'
            | '因'
            | '所'
            | '以'
            | '中'
            | '上'
            | '下'
            | '内'
            | '外'
            | '前'
            | '后'
            | '个'
            | '等'
            | '将'
            | '由'
            | '向'
            | '往'
            | '到'
            | '让'
            | '使'
            | '给'
            | '这'
            | '那'
            | '其'
            | '每'
            | '各'
            | '该'
            | '本'
    )
}

fn is_stop_word(token: &str) -> bool {
    STOP_WORDS.contains(&token)
}

/// Tokenize chunk text into keyword candidates.
///
/// Returns `(token, char_offset)` pairs so position scoring uses the true
/// character position (CJK span fragments starting at the same offset share
/// the same position boost).
///
/// - ASCII/alphanumeric runs become single lowercase word tokens.
/// - CJK runs emit every contiguous 2..=6-char span plus the whole run
///   (coarse jieba-like phrase candidates, e.g. `机器学习` and its parts).
///
/// Stop words and pure-digit tokens are dropped.
fn tokenize(text: &str) -> Vec<(String, usize)> {
    let mut tokens: Vec<(String, usize)> = Vec::new();
    let mut alpha = String::new();
    let mut alpha_start = 0usize;
    let mut cjk_run: Vec<char> = Vec::new();
    let mut cjk_start = 0usize;

    let flush_alpha = |alpha: &mut String, start: &mut usize, out: &mut Vec<(String, usize)>| {
        let word = alpha.to_ascii_lowercase();
        alpha.clear();
        let offset = *start;
        *start = 0;
        if word.is_empty() || word.chars().all(|c| c.is_ascii_digit()) || is_stop_word(&word) {
            return;
        }
        out.push((word, offset));
    };
    let flush_cjk = |run: &mut Vec<char>, start: &mut usize, out: &mut Vec<(String, usize)>| {
        if run.is_empty() {
            return;
        }
        let run_start = *start;
        *start = 0;
        let max_span = run.len().min(6);
        if run.len() >= 2 {
            for span_start in 0..run.len() {
                // 跳过以虚词开头的片段（如「的重要」「是人工」）。
                if is_cjk_function_char(run[span_start]) {
                    continue;
                }
                for span_len in 2..=max_span.min(run.len() - span_start) {
                    let span: String = run[span_start..span_start + span_len].iter().collect();
                    out.push((span, run_start + span_start));
                }
            }
        }
        // 单字 CJK（多为虚词/助词，如「是」「的」）不作为候选；
        // 整段 run 也无需单独入列（长度 ≤ 6 时已被片段覆盖，更长时只是整句噪音）。
        run.clear();
    };

    let mut offset = 0usize;
    for ch in text.chars() {
        if is_cjk(ch) {
            flush_alpha(&mut alpha, &mut alpha_start, &mut tokens);
            if cjk_run.is_empty() {
                cjk_start = offset;
            }
            cjk_run.push(ch);
        } else if ch.is_alphanumeric() {
            flush_cjk(&mut cjk_run, &mut cjk_start, &mut tokens);
            if alpha.is_empty() {
                alpha_start = offset;
            }
            alpha.push(ch);
        } else {
            flush_alpha(&mut alpha, &mut alpha_start, &mut tokens);
            flush_cjk(&mut cjk_run, &mut cjk_start, &mut tokens);
        }
        offset += 1;
    }
    flush_alpha(&mut alpha, &mut alpha_start, &mut tokens);
    flush_cjk(&mut cjk_run, &mut cjk_start, &mut tokens);
    tokens
}

/// POS-proxy weight (词性统计).
///
/// Without a tagger, approximate jieba's noun-heavy keyword behavior:
/// - terms containing CJK characters are treated as noun-like → 1.2;
/// - capitalized alpha terms are treated as proper nouns → 1.3;
/// - all-lowercase generic alpha terms → 0.9;
/// - anything else → 1.0.
fn pos_factor(term: &str) -> f32 {
    if term.chars().any(is_cjk) {
        1.2
    } else {
        let first = term.chars().next().unwrap_or(' ');
        if first.is_ascii_uppercase() && term.len() > 1 {
            1.3
        } else if term.chars().all(|c| c.is_ascii_alphabetic()) {
            0.9
        } else {
            1.0
        }
    }
}

/// Length factor: longer terms are better keywords.
fn length_factor(term: &str) -> f32 {
    match term.chars().count() {
        0 | 1 => 0.7,
        2 => 1.0,
        3 => 1.1,
        _ => 1.2,
    }
}

/// Position factor: 1 + 1/(1 + first_offset) ∈ (1.0, 2.0].
fn position_factor(first_offset: usize) -> f32 {
    1.0 + 1.0 / (1.0 + first_offset as f32)
}

/// Candidate term stats accumulated over the token stream.
struct Candidate {
    /// Raw term frequency.
    tf: usize,
    /// Character offset of the first occurrence in the text.
    first_offset: usize,
    /// Bonus for phrasal (bigram) candidates.
    phrasal: bool,
}

/// Extract key terms from `text`, returning the top `top_n` ranked terms.
///
/// Ranking combines term frequency, first-occurrence position, a POS proxy
/// and term length. Phrases (adjacent token bigrams) compete with unigrams.
pub fn extract_key_terms_with_top_n(text: &str, top_n: usize) -> Vec<String> {
    let tokens = tokenize(text);
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut stats: HashMap<String, Candidate> = HashMap::new();
    let mut record = |term: &str, offset: usize, phrasal: bool| {
        let entry = stats.entry(term.to_string()).or_insert(Candidate {
            tf: 0,
            first_offset: offset,
            phrasal,
        });
        entry.tf += 1;
        if offset < entry.first_offset {
            entry.first_offset = offset;
        }
    };

    for (token, offset) in &tokens {
        record(token, *offset, false);
    }
    for pair in tokens.windows(2) {
        // Skip phrases that start with a stop word — the stop word still
        // contributes noise to the phrase (e.g. "the system").
        if is_stop_word(&pair[0].0) || is_stop_word(&pair[1].0) {
            continue;
        }
        // CJK span tokens already ARE phrase candidates; adjacent bigrams
        // over overlapping spans only add noise (e.g. "机器 机器学").
        if pair[0].0.chars().any(is_cjk) || pair[1].0.chars().any(is_cjk) {
            continue;
        }
        let phrase = format!("{} {}", pair[0].0, pair[1].0);
        record(&phrase, pair[0].1, true);
    }

    let mut scored: Vec<(f32, String)> = stats
        .into_iter()
        .map(|(term, candidate)| {
            let mut score = candidate.tf as f32
                * position_factor(candidate.first_offset)
                * length_factor(&term)
                * pos_factor(&term);
            if candidate.phrasal {
                score *= 1.1;
            }
            (score, term)
        })
        .collect();

    // Highest score first; ties broken by lexicographic order for stability.
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.truncate(top_n.max(1));
    scored.into_iter().map(|(_, term)| term).collect()
}

/// Extract key terms from `text` with the default top-N.
pub fn extract_key_terms(text: &str) -> Vec<String> {
    extract_key_terms_with_top_n(text, DEFAULT_TOP_N)
}

/// Offline key-term extractor used by the parsing pipeline.
#[derive(Debug, Clone)]
pub struct Extractor {
    top_n: usize,
}

impl Extractor {
    pub fn new() -> Self {
        Self {
            top_n: DEFAULT_TOP_N,
        }
    }

    /// Create an extractor returning at most `top_n` terms per chunk.
    pub fn with_top_n(top_n: usize) -> Self {
        Self {
            top_n: top_n.max(1),
        }
    }

    /// Extract key terms from a single text.
    pub fn extract(&self, text: &str) -> Vec<String> {
        extract_key_terms_with_top_n(text, self.top_n)
    }

    /// Annotate each chunk with its extracted key terms.
    ///
    /// Mirrors RAGFlow `task_executor.py`: keywords are stored on the chunk
    /// as the `important_kwd` field, consumed by hybrid retrieval with an
    /// elevated BM25 weight (`search.rs`). The join format is a single
    /// space, matching `api/chunks.rs set_metadata_list`.
    pub fn extract_chunks(&self, chunks: &mut [Chunk]) {
        for chunk in chunks.iter_mut() {
            if chunk.content.trim().is_empty() {
                continue;
            }
            let terms = extract_key_terms_with_top_n(&chunk.content, self.top_n);
            if terms.is_empty() {
                continue;
            }
            chunk
                .metadata
                .insert("important_kwd".to_string(), terms.join(" "));
        }
    }
}

impl Default for Extractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn chunk(content: &str) -> Chunk {
        Chunk {
            id: "c".into(),
            content: content.into(),
            content_type: "text".into(),
            doc_id: Uuid::new_v4(),
            position: 0,
            token_count: 0,
            embedding: None,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn extract_key_terms_ranks_frequent_terms_first() {
        // "database" dominates the text → must be the top term.
        let text = "The database design keeps the database fast and the \
                    database scalable; this database also stays consistent.";
        let terms = extract_key_terms(text);
        assert!(!terms.is_empty());
        assert_eq!(terms[0], "database");
        // 结果不超上限且不含停用词。
        assert!(terms.len() <= 10);
        assert!(!terms.iter().any(|t| t == "the" || t == "and"));
    }

    #[test]
    fn extract_key_terms_boosts_early_terms_and_noun_like_terms() {
        // "Retrieval" appears once, at the very start, and is capitalized
        // (proper-noun proxy). A stop word is excluded and digits dropped.
        let text = "Retrieval augmented generation; retrieval quality matters.";
        let terms = extract_key_terms(text);
        assert!(terms.contains(&"retrieval".to_string()));
        for term in &terms {
            assert_ne!(term, "the");
            assert_ne!(term, "and");
            assert!(!term.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn extract_key_terms_handles_cjk_text() {
        let text = "机器学习是人工智能的重要分支。深度学习也是机器学习的重要分支。";
        let terms = extract_key_terms(text);
        assert!(!terms.is_empty());
        // 机器学习 appears twice and is a full-run CJK phrase → top term.
        assert_eq!(terms[0], "机器学习");
        eprintln!("CJK_TERMS {:?}", terms.iter().take(15).collect::<Vec<_>>());
        // 2 字片段与 4 字短语都进入前排（重要/分支 tf=2，排名确定）。
        assert!(terms.iter().take(5).any(|t| t == "学习"));
        assert!(
            terms
                .iter()
                .take(12)
                .any(|t| t.contains("重要") && t.contains("分支"))
        );
        // 4 字短语（人工智能）也能被提取（用大 top_n 避免默认截断遮蔽）。
        let wide = extract_key_terms_with_top_n(text, 60);
        assert!(wide.iter().any(|t| t.contains("人工智能")));
        // CJK 片段不会拼出带空格的噪音二元组。
        assert!(!terms.iter().any(|t| t.contains(' ')));
    }

    #[test]
    fn extract_chunks_annotates_important_kwd_metadata() {
        let text = "Vector search indexes vectors; vector search over vectors is fast.";
        let mut chunks = vec![chunk(text)];
        Extractor::with_top_n(3).extract_chunks(&mut chunks);
        let keywords = chunks[0]
            .metadata
            .get("important_kwd")
            .expect("important_kwd metadata set");
        assert!(keywords.contains("vector"));
        assert!(keywords.contains("search"));
        // top_n=3 个词条；keywords 是同一组词条的空格拼接（短语含内部空格）。
        let terms = extract_key_terms_with_top_n(text, 3);
        assert_eq!(terms.len(), 3);
        assert_eq!(keywords.as_str(), terms.join(" "));
    }

    #[test]
    fn extractor_skips_empty_chunks() {
        let mut chunks = vec![chunk("   \n  "), chunk("alpha beta alpha")];
        Extractor::with_top_n(5).extract_chunks(&mut chunks);
        assert!(chunks[0].metadata.get("important_kwd").is_none());
        assert!(chunks[1].metadata.get("important_kwd").is_some());
    }
}
