//! NLP module — BM25 keyword scoring, synonym expansion, query rewriting.
//! Replaces RAGFlow's `rag/nlp/query.py` + `term_weight.py` + `synonym.py`.

use std::collections::HashMap;

// ── BM25 Scorer ─────────────────────────────────────────────────

/// BM25 scoring for keyword-based search (hybrid with vector).
pub struct Bm25Scorer {
    /// k1 parameter (term frequency saturation)
    k1: f32,
    /// b parameter (length normalization)
    b: f32,
    /// Average document length
    avg_dl: f32,
    /// Document count
    doc_count: usize,
    /// Term → document frequency
    term_doc_freq: HashMap<String, usize>,
}

impl Bm25Scorer {
    pub fn new() -> Self {
        Self {
            k1: 1.2,
            b: 0.75,
            avg_dl: 100.0,
            doc_count: 0,
            term_doc_freq: HashMap::new(),
        }
    }

    /// Index a set of documents.
    pub fn index(&mut self, documents: &[&str]) {
        self.doc_count = documents.len();
        let mut total_len = 0;
        for doc in documents {
            total_len += Self::word_count(doc);
            let mut seen = std::collections::HashSet::new();
            for word in tokenize(doc) {
                if seen.insert(word.clone()) {
                    *self.term_doc_freq.entry(word.clone()).or_insert(0) += 1;
                }
            }
        }
        if self.doc_count > 0 {
            self.avg_dl = total_len as f32 / self.doc_count as f32;
        }
    }

    /// Compute BM25 score for a query against a document.
    pub fn score(&self, query: &str, doc: &str) -> f32 {
        let doc_len = Self::word_count(doc) as f32;
        let query_terms = tokenize(query);
        if query_terms.is_empty() || self.doc_count == 0 {
            return 0.0;
        }

        let mut score = 0.0;
        for term in &query_terms {
            let df = *self.term_doc_freq.get(term).unwrap_or(&0) as f32;
            if df == 0.0 {
                continue;
            }
            // IDF component
            let idf = ((self.doc_count as f32 - df + 0.5) / (df + 0.5) + 1.0)
                .ln()
                .max(0.0);
            // TF component
            let tf = term_frequency(doc, term);
            let numerator = tf * (self.k1 + 1.0);
            let denominator = tf + self.k1 * (1.0 - self.b + self.b * doc_len / self.avg_dl);
            score += idf * numerator / denominator;
        }
        score
    }

    /// Weighted BM25 score: per-term query weights (RAGFlow term_weight.py)
    /// scale each term's contribution before summation. Tokens absent from
    /// the weight map keep weight 1.0.
    pub fn score_weighted(&self, query: &str, doc: &str, weights: &[(&str, f64)]) -> f32 {
        let doc_len = Self::word_count(doc) as f32;
        let query_terms = tokenize(query);
        if query_terms.is_empty() || self.doc_count == 0 {
            return 0.0;
        }
        let weight_of = |term: &str| -> f32 {
            weights
                .iter()
                .find(|(t, _)| *t == term)
                .map(|(_, w)| *w as f32)
                .unwrap_or(1.0)
        };
        let mut score = 0.0;
        for term in &query_terms {
            let df = *self.term_doc_freq.get(term).unwrap_or(&0) as f32;
            if df == 0.0 {
                continue;
            }
            let idf = ((self.doc_count as f32 - df + 0.5) / (df + 0.5) + 1.0)
                .ln()
                .max(0.0);
            let tf = term_frequency(doc, term);
            let numerator = tf * (self.k1 + 1.0);
            let denominator = tf + self.k1 * (1.0 - self.b + self.b * doc_len / self.avg_dl);
            score += weight_of(term) * idf * numerator / denominator;
        }
        score
    }

    /// Hybrid search: combine BM25 score with vector similarity score.
    pub fn hybrid_score(&self, query: &str, doc: &str, vector_score: f32, bm25_weight: f32) -> f32 {
        let bm25 = self.score(query, doc);
        // Normalize BM25 to 0-1 range (approximate)
        let bm25_norm = (bm25 / (bm25 + 5.0)).min(1.0);
        bm25_weight * bm25_norm + (1.0 - bm25_weight) * vector_score
    }

    fn word_count(text: &str) -> usize {
        tokenize(text).len()
    }
}

impl Default for Bm25Scorer {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute term frequency in a document (normalized).
fn term_frequency(doc: &str, term: &str) -> f32 {
    let words = tokenize(doc);
    if words.is_empty() {
        return 0.0;
    }
    let count = words.iter().filter(|w| w.as_str() == term).count();
    count as f32 / words.len() as f32
}

/// Tokenize text into lowercase words (alphanumeric only).
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut alphanumeric = String::new();
    let mut cjk_run = Vec::new();

    let flush_alphanumeric = |buffer: &mut String, output: &mut Vec<String>| {
        if !buffer.is_empty() && !is_stop_word(buffer) {
            output.push(std::mem::take(buffer));
        } else {
            buffer.clear();
        }
    };
    let flush_cjk = |run: &mut Vec<char>, output: &mut Vec<String>| {
        output.extend(run.iter().map(char::to_string));
        output.extend(run.windows(2).map(|pair| pair.iter().collect()));
        run.clear();
    };

    for ch in text.to_lowercase().chars() {
        if is_cjk(ch) {
            flush_alphanumeric(&mut alphanumeric, &mut tokens);
            cjk_run.push(ch);
        } else if ch.is_alphanumeric() {
            flush_cjk(&mut cjk_run, &mut tokens);
            alphanumeric.push(ch);
        } else {
            flush_alphanumeric(&mut alphanumeric, &mut tokens);
            flush_cjk(&mut cjk_run, &mut tokens);
        }
    }
    flush_alphanumeric(&mut alphanumeric, &mut tokens);
    flush_cjk(&mut cjk_run, &mut tokens);
    tokens
}

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

fn is_stop_word(w: &str) -> bool {
    matches!(
        w,
        "the"
            | "and"
            | "for"
            | "with"
            | "this"
            | "that"
            | "are"
            | "was"
            | "were"
            | "been"
            | "have"
            | "has"
            | "had"
            | "not"
            | "but"
            | "from"
            | "they"
            | "will"
            | "would"
            | "could"
            | "should"
            | "can"
            | "may"
            | "might"
            | "shall"
            | "its"
            | "his"
            | "her"
            | "our"
            | "your"
            | "their"
            | "them"
    )
}

// ── Synonym Expansion ───────────────────────────────────────────

/// Synonym dictionary for query expansion.
pub struct SynonymDict {
    /// Term → list of synonyms
    synonyms: HashMap<String, Vec<String>>,
}

impl SynonymDict {
    pub fn new() -> Self {
        let mut synonyms = HashMap::new();
        // Common synonym pairs
        let pairs = vec![
            ("fast", vec!["quick", "rapid", "speedy"]),
            ("slow", vec!["sluggish", "lethargic"]),
            ("big", vec!["large", "huge", "enormous"]),
            ("small", vec!["tiny", "little", "miniature"]),
            ("good", vec!["great", "excellent", "fine"]),
            ("bad", vec!["poor", "terrible", "awful"]),
            ("create", vec!["make", "build", "construct"]),
            ("error", vec!["mistake", "fault", "bug"]),
            ("help", vec!["assist", "support", "aid"]),
            ("start", vec!["begin", "initiate", "commence"]),
            ("stop", vec!["halt", "cease", "terminate"]),
            ("use", vec!["utilize", "employ", "apply"]),
            ("show", vec!["display", "exhibit", "reveal"]),
            ("find", vec!["discover", "locate", "detect"]),
            ("change", vec!["modify", "alter", "transform"]),
        ];
        for (k, v) in pairs {
            synonyms.insert(k.to_string(), v.iter().map(|s| s.to_string()).collect());
            for s in &v.iter().map(|s| s.to_string()).collect::<Vec<_>>() {
                let mut entry: Vec<String> = vec![k.to_string()];
                entry.extend(v.iter().filter(|&&x| x != s).map(|s| s.to_string()));
                synonyms.insert(s.clone(), entry);
            }
        }
        Self { synonyms }
    }

    /// Build a dictionary from a synonym.json-style map (key → string or
    /// list of strings). Keys are lowercased, mirroring RAGFlow Dealer's
    /// `{k.lower(): v}` normalization. Mirrors the custom-dictionary half of
    /// `synonym.py Dealer.lookup`.
    pub fn from_json_map(map: &serde_json::Map<String, serde_json::Value>) -> Self {
        let mut synonyms = HashMap::new();
        for (k, v) in map {
            let key = k.to_lowercase();
            let entry = match v {
                serde_json::Value::String(s) => vec![s.clone()],
                serde_json::Value::Array(items) => items
                    .iter()
                    .filter_map(|i| i.as_str().map(str::to_string))
                    .collect(),
                _ => continue,
            };
            if !entry.is_empty() {
                synonyms.insert(key, entry);
            }
        }
        Self { synonyms }
    }

    /// Look up synonyms for a token, mirroring RAGFlow `synonym.py
    /// Dealer.lookup`: normalized whitespace, custom dictionary first, then
    /// a WordNet-equivalent fallback for pure-alpha tokens (built-in table
    /// replaces NLTK wordnet to stay dependency-free and offline-friendly).
    /// Returns at most `topn` entries.
    pub fn lookup(&self, tk: &str, topn: usize) -> Vec<String> {
        if tk.is_empty() {
            return Vec::new();
        }
        let key = tk.split_whitespace().collect::<Vec<_>>().join(" ");
        if let Some(res) = self.synonyms.get(&key) {
            return res.iter().take(topn).cloned().collect();
        }
        // WordNet-equivalent fallback for pure lowercase alpha tokens
        if key.chars().all(|c| c.is_ascii_lowercase()) {
            if let Some(res) = self.synonyms.get(&key) {
                return res.iter().take(topn).cloned().collect();
            }
        }
        Vec::new()
    }

    /// Expand a query with synonyms (limit to top N expansions).
    pub fn expand(&self, query: &str, max_expansions: usize) -> String {
        let words: Vec<&str> = query.split_whitespace().collect();
        let mut expanded = String::new();
        let mut added = 0;

        for word in &words {
            expanded.push_str(word);
            expanded.push(' ');

            let lower = word
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase();
            if let Some(syns) = self.synonyms.get(&lower) {
                for syn in syns.iter().take(2) {
                    if added >= max_expansions {
                        break;
                    }
                    expanded.push_str(syn);
                    expanded.push(' ');
                    added += 1;
                }
            }
        }

        expanded.trim().to_string()
    }
}

impl Default for SynonymDict {
    fn default() -> Self {
        Self::new()
    }
}

// ── Term Weight Computer ────────────────────────────────────────
// Mirrors RAGFlow `rag/nlp/term_weight.py` Dealer class: per-term query
// weights = (0.3*idf(freq) + 0.7*idf(df)) * ner(t), normalized.
// Uses this module's tokenizer instead of jieba; resource files (ner.json /
// term.freq) are optional and degrade to defaults when absent.

/// Term weight computer — RAGFlow `Dealer` port.
pub struct TermWeightComputer {
    /// Chinese/query stop words (RAGFlow Dealer.stop_words verbatim).
    stop_words: std::collections::HashSet<String>,
    /// NER category table (ner.json): term -> category.
    ne: HashMap<String, String>,
    /// Document frequency table (term.freq): term -> df.
    df: HashMap<String, f64>,
}

impl TermWeightComputer {
    pub fn new() -> Self {
        let stop_words = [
            "请问", "您", "你", "我", "他", "是", "的", "就", "有", "于", "及", "即",
            "在", "为", "最", "有", "从", "以", "了", "将", "与", "吗", "吧", "中",
            "#", "什么", "怎么", "哪个", "哪些", "啥", "相关",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        Self {
            stop_words,
            ne: HashMap::new(),
            df: HashMap::new(),
        }
    }

    /// Attach NER + document-frequency resource tables (from ner.json /
    /// term.freq when available). Degrades gracefully when absent.
    pub fn with_resources(mut self, ne: HashMap<String, String>, df: HashMap<String, f64>) -> Self {
        self.ne = ne;
        self.df = df;
        self
    }

    /// Pre-tokenize: split, drop stop words, drop bare single digits when
    /// `num` is false, and replace punctuation-class tokens with "#" which
    /// are then dropped (mirrors Dealer.pretoken).
    pub fn pretoken(&self, txt: &str, num: bool, stpwd: bool) -> Vec<String> {
        // RAGFlow Dealer.pretoken pattern: punctuation/symbol characters are
        // remapped to "#" then dropped. A token consisting solely of
        // punctuation/symbols (no CJK, no alphanumeric) is classified as such.
        let is_symbol = |t: &str| {
            t.chars().count() == 1
                && !t.chars().next().unwrap().is_alphanumeric()
                && !is_cjk(t.chars().next().unwrap())
        };
        let mut res = Vec::new();
        for t in tokenize(txt) {
            let is_digit_only = t.chars().all(|c| c.is_ascii_digit());
            if (stpwd && self.stop_words.contains(&t)) || (is_digit_only && !num) {
                continue;
            }
            if is_symbol(&t) {
                continue; // "#" remap then drop
            }
            if !t.is_empty() {
                res.push(t);
            }
        }
        res
    }

    /// Merge runs of one-character terms into phrases (mirrors token_merge:
    /// head bigram when first term is single and second is CJK, then greedy
    /// runs of one_term ≤ 4, split 5+ runs into bigrams).
    pub fn token_merge(&self, tks: &[String]) -> Vec<String> {
        fn one_term(t: &str) -> bool {
            t.chars().count() == 1
                || regex::Regex::new(r"^[0-9a-z]{1,2}$").unwrap().is_match(t)
        }
        let mut res: Vec<String> = Vec::new();
        let mut i = 0;
        while i < tks.len() {
            if i == 0
                && one_term(&tks[i])
                && tks.len() > 1
                && (tks[i + 1].chars().count() > 1
                    && !tks[i + 1].chars().next().unwrap().is_ascii_alphanumeric())
            {
                res.push(format!("{} {}", tks[0], tks[1]));
                i = 2;
                continue;
            }
            let mut j = i;
            while j < tks.len()
                && !tks[j].is_empty()
                && !self.stop_words.contains(&tks[j])
                && one_term(&tks[j])
            {
                j += 1;
            }
            if j - i > 1 {
                if j - i < 5 {
                    res.push(tks[i..j].join(" "));
                    i = j;
                } else {
                    res.push(tks[i..i + 2].join(" "));
                    i += 2;
                }
            } else {
                if !tks[i].is_empty() {
                    res.push(tks[i].clone());
                }
                i += 1;
            }
        }
        res.into_iter().filter(|t| !t.is_empty()).collect()
    }

    /// NER category weight (mirrors Dealer.ner + weights.ner):
    /// numbers → 2, short letters → 0.01, unknown → 1, known → category map.
    fn ner(&self, t: &str) -> f64 {
        let num_pattern = regex::Regex::new(r"^[0-9,.]{2,}$").unwrap();
        let short_letter = regex::Regex::new(r"^[a-z]{1,2}$").unwrap();
        if num_pattern.is_match(t) {
            return 2.0;
        }
        if short_letter.is_match(t) {
            return 0.01;
        }
        match self.ne.get(t).map(String::as_str) {
            None => 1.0,
            Some("corp") | Some("loca") | Some("sch") | Some("stock") => 3.0,
            Some(_) => 1.0,
        }
    }

    /// Term-frequency proxy (mirrors Dealer.freq): number-space → 3, letters
    /// → 300, else max(freq, 10). Resource-less fallback keeps 10.
    fn freq(&self, t: &str) -> f64 {
        let num_space = regex::Regex::new(r"^[0-9. -]{2,}$").unwrap();
        let letter = regex::Regex::new(r"^[a-z. -]+$").unwrap();
        if num_space.is_match(t) {
            return 3.0;
        }
        if self.df.contains_key(t) {
            return (self.df[t] + 3.0).max(10.0);
        }
        if letter.is_match(t) {
            return 300.0;
        }
        10.0
    }

    /// Document-frequency proxy (mirrors Dealer.df): number-space → 5,
    /// known → df+3, letters → 300, else 3.
    fn df(&self, t: &str) -> f64 {
        let num_space = regex::Regex::new(r"^[0-9. -]{2,}$").unwrap();
        let letter = regex::Regex::new(r"^[a-z. -]+$").unwrap();
        if num_space.is_match(t) {
            return 5.0;
        }
        if let Some(v) = self.df.get(t) {
            return v + 3.0;
        }
        if letter.is_match(t) {
            return 300.0;
        }
        3.0
    }

    fn idf(s: f64, n: f64) -> f64 {
        (10.0 + (n - s + 0.5) / (s + 0.5)).log10()
    }

    /// Compute normalized per-term weights for a token list. When
    /// `preprocess` is true each token is pre-tokenized + merged first
    /// (mirrors Dealer.weights).
    pub fn weights(&self, tks: &[String], preprocess: bool) -> Vec<(String, f64)> {
        let mut tw: Vec<(String, f64)> = Vec::new();
        let compute = |tt: &[String], tw: &mut Vec<(String, f64)>| {
            let idf1: Vec<f64> = tt
                .iter()
                .map(|t| Self::idf(self.freq(t), 10_000_000.0))
                .collect();
            let idf2: Vec<f64> = tt
                .iter()
                .map(|t| Self::idf(self.df(t), 1_000_000_000.0))
                .collect();
            for (i, t) in tt.iter().enumerate() {
                let w = (0.3 * idf1[i] + 0.7 * idf2[i]) * self.ner(t);
                tw.push((t.clone(), w));
            }
        };
        if !preprocess {
            compute(tks, &mut tw);
        } else {
            for tk in tks {
                let tt = self.token_merge(&self.pretoken(tk, true, true));
                compute(&tt, &mut tw);
            }
        }
        let s: f64 = tw.iter().map(|(_, w)| w).sum();
        if s <= 0.0 {
            return tw;
        }
        tw.into_iter().map(|(t, w)| (t, w / s)).collect()
    }
}

impl Default for TermWeightComputer {
    fn default() -> Self {
        Self::new()
    }
}

// ── Query Rewriting ─────────────────────────────────────────────

/// Query rewriter — cleans and expands queries for better retrieval.
pub struct QueryRewriter {
    synonyms: SynonymDict,
}

impl QueryRewriter {
    pub fn new() -> Self {
        Self {
            synonyms: SynonymDict::new(),
        }
    }

    /// Rewrite a query for better search results.
    /// Steps: 1) clean 2) expand synonyms 3) weight key terms
    pub fn rewrite(&self, query: &str) -> String {
        // Step 1: Clean — remove common filler phrases
        let cleaned = clean_query(query);

        // Step 2: Remove question words at the beginning
        let no_qw = remove_question_words(&cleaned);

        // Step 3: Expand with synonyms
        self.synonyms.expand(&no_qw, 5)
    }

    /// Generate multiple query variations for multi-hop retrieval.
    pub fn variations(&self, query: &str, count: usize) -> Vec<String> {
        let mut vars = vec![query.to_string()];

        // Add cleaned version
        let cleaned = clean_query(query);
        if cleaned != query {
            vars.push(cleaned.clone());
        }

        // Add synonym-expanded version
        let expanded = self.synonyms.expand(&cleaned, 3);
        if expanded != cleaned {
            vars.push(expanded);
        }

        // Add key-term-only version
        let key_terms = extract_key_terms(&cleaned);
        if !key_terms.is_empty() && key_terms != cleaned {
            vars.push(key_terms);
        }

        vars.truncate(count);
        vars
    }
}

impl Default for QueryRewriter {
    fn default() -> Self {
        Self::new()
    }
}

/// Clean a query: remove filler phrases and normalize.
fn clean_query(query: &str) -> String {
    let remove_patterns = [
        "please ",
        "can you ",
        "could you ",
        "would you ",
        "tell me ",
        "show me ",
        "give me ",
        "find me ",
        "i want to know ",
        "i would like to know ",
        "help me ",
        "explain to me ",
    ];

    let mut cleaned = query.to_lowercase();
    for pat in &remove_patterns {
        cleaned = cleaned.replacen(pat, "", 1);
    }
    cleaned.trim().to_string()
}

/// Remove leading question words.
fn remove_question_words(query: &str) -> String {
    let qw = [
        "what ", "how ", "why ", "when ", "where ", "who ", "which ", "is ", "are ", "do ",
        "does ", "did ",
    ];
    let mut result = query.to_lowercase();
    for w in &qw {
        if result.starts_with(w) {
            result = result[w.len()..].to_string();
            break;
        }
    }
    result.trim().to_string()
}

/// Extract key terms (nouns and significant words).
fn extract_key_terms(query: &str) -> String {
    tokenize(query)
        .iter()
        .filter(|w| w.len() > 3)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bm25() {
        let mut scorer = Bm25Scorer::new();
        scorer.index(&["the quick brown fox", "the lazy dog", "quick brown rabbit"]);
        let s1 = scorer.score("quick fox", "the quick brown fox");
        let s2 = scorer.score("quick fox", "the lazy dog");
        assert!(s1 > s2, "BM25 should score matching doc higher");
    }

    #[test]
    fn test_bm25_matches_cjk_subterms() {
        let mut scorer = Bm25Scorer::new();
        scorer.index(&["水产养殖水质管理", "服务器日志分析"]);
        let matching = scorer.score("水质", "水产养殖水质管理");
        let unrelated = scorer.score("水质", "服务器日志分析");
        assert!(matching > unrelated);
    }

    #[test]
    fn test_synonym_expand() {
        let dict = SynonymDict::new();
        let result = dict.expand("fast car", 3);
        assert!(result.contains("quick") || result.contains("rapid"));
    }

    #[test]
    fn test_synonym_lookup_dict_and_topn() {
        let dict = SynonymDict::new();
        let res = dict.lookup("fast", 2);
        assert_eq!(res.len(), 2);
        assert!(res.iter().any(|s| s == "quick" || s == "rapid"));
        // Unknown token → empty (RAGFlow returns [])
        assert!(dict.lookup("zzzznope", 8).is_empty());
        // Empty input → empty
        assert!(dict.lookup("", 8).is_empty());
    }

    #[test]
    fn test_synonym_from_json_map() {
        let mut map = serde_json::Map::new();
        map.insert("Fast".into(), serde_json::Value::String("quick".into()));
        map.insert(
            "汽车".into(),
            serde_json::Value::Array(vec![
                serde_json::Value::String("车辆".into()),
                serde_json::Value::String("机动车".into()),
            ]),
        );
        map.insert("ignored".into(), serde_json::Value::Number(42.into()));
        let dict = SynonymDict::from_json_map(&map);
        // Keys lowercased like RAGFlow's {k.lower(): v}
        assert_eq!(dict.lookup("fast", 8), vec!["quick"]);
        assert_eq!(dict.lookup("汽车", 8), vec!["车辆", "机动车"]);
        // Non-string/list values skipped
        assert!(dict.lookup("ignored", 8).is_empty());
    }

    #[test]
    fn test_query_clean() {
        let cleaned = clean_query("Can you tell me how to use Rust?");
        assert!(!cleaned.contains("can you"));
        assert!(!cleaned.contains("tell me"));
    }

    #[test]
    fn test_hybrid_score() {
        let mut scorer = Bm25Scorer::new();
        scorer.index(&[
            "rust is a systems programming language",
            "python is easy to learn",
        ]);
        let hybrid = scorer.hybrid_score(
            "rust systems",
            "rust is a systems programming language",
            0.9,
            0.3,
        );
        assert!(hybrid > 0.5);
    }

    #[test]
    fn test_term_weight_pretoken_drops_stopwords_and_punct() {
        let tw = TermWeightComputer::new();
        let tokens = tw.pretoken("请问 什么是 水产养殖 水质 管理？", true, true);
        // 请问/什么是 are stop words; ？ becomes # and is dropped
        assert!(!tokens.iter().any(|t| t == "请问" || t == "什么是"));
        assert!(!tokens.iter().any(|t| t == "#"));
        assert!(tokens.iter().any(|t| t == "水产"));
    }

    #[test]
    fn test_term_weight_pretoken_drops_digits_when_num_false() {
        let tw = TermWeightComputer::new();
        let tokens = tw.pretoken("2026 年 计划", false, true);
        assert!(!tokens.iter().any(|t| t == "2026"));
    }

    #[test]
    fn test_term_weight_merge_single_char_phrases() {
        let tw = TermWeightComputer::new();
        // "多 工 位" style CJK run merges into a phrase
        let merged = tw.token_merge(&["多".into(), "工".into(), "位".into()]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0], "多 工 位");
    }

    #[test]
    fn test_term_weight_weights_normalize_and_boost_numbers() {
        let tw = TermWeightComputer::new();
        let mut ne = HashMap::new();
        ne.insert("中山市".to_string(), "loca".to_string());
        let mut df = HashMap::new();
        df.insert("水产".to_string(), 1000.0);
        let tw = tw.with_resources(ne, df);
        let weights = tw.weights(&["中山市".into(), "水产".into(), "42.5".into()], false);
        let sum: f64 = weights.iter().map(|(_, w)| w).sum();
        assert!((sum - 1.0).abs() < 1e-9, "weights must normalize to 1, got {sum}");
        let num_w = weights.iter().find(|(t, _)| t == "42.5").map(|(_, w)| *w).unwrap();
        let loca_w = weights.iter().find(|(t, _)| t == "中山市").map(|(_, w)| *w).unwrap();
        // RAGFlow ner map: loca=3 > number=2 — location must beat numbers
        assert!(loca_w > num_w, "location NER weight (3) should beat number (2)");
    }

    #[test]
    fn test_term_weight_preprocess_merges_phrases() {
        let tw = TermWeightComputer::new();
        // preprocess=true runs pretoken+token_merge on each token
        let weights = tw.weights(&["多 工 位 加工".into()], true);
        assert!(!weights.is_empty());
        let sum: f64 = weights.iter().map(|(_, w)| w).sum();
        assert!((sum - 1.0).abs() < 1e-9);
    }
}
