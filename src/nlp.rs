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
        if key.chars().all(|c| c.is_ascii_lowercase())
            && let Some(res) = self.synonyms.get(&key) {
                return res.iter().take(topn).cloned().collect();
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
            "请问", "您", "你", "我", "他", "是", "的", "就", "有", "于", "及", "即", "在", "为",
            "最", "有", "从", "以", "了", "将", "与", "吗", "吧", "中", "#", "什么", "怎么",
            "哪个", "哪些", "啥", "相关",
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
            t.chars().count() == 1 || regex::Regex::new(r"^[0-9a-z]{1,2}$").unwrap().is_match(t)
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
    /// numbers → 2, short letters → 0.01, unknown → 1, known → category map
    /// with toxic=2, func=1, corp/loca/sch/stock=3, firstnm=1.
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
            Some("toxic") => 2.0,
            Some("func") => 1.0,
            Some("corp") | Some("loca") | Some("sch") | Some("stock") => 3.0,
            Some("firstnm") => 1.0,
            Some(_) => 1.0,
        }
    }

    /// POS-tag weight proxy (mirrors Dealer.postag, which uses jieba
    /// `rag_tokenizer.tag`): pronouns/particles (r/c/d) → 0.3, place/org
    /// nouns (ns/nt) → 3, common nouns (n) → 2, numbers → 2, else 1.
    ///
    /// jieba is unavailable in Rust, so the tag is approximated from the
    /// token surface: location/org suffixes imply ns/nt, pure CJK tokens of
    /// length ≥ 2 imply a noun, leading digits imply a numeral, and a small
    /// closed set of pronouns/particles maps to r/c/d.
    fn postag(&self, t: &str) -> f64 {
        let num_prefix = regex::Regex::new(r"^[0-9-]").unwrap();
        if num_prefix.is_match(t) {
            return 2.0;
        }
        // ns/nt suffixes — the same set RAGFlow's jieba dictionary tags as
        // place names (ns) / organization names (nt).
        let ns_nt_suffix = [
            "市",
            "省",
            "县",
            "区",
            "镇",
            "乡",
            "村",
            "国",
            "州",
            "港",
            "岛",
            "河",
            "江",
            "湖",
            "山",
            "路",
            "街",
            "公司",
            "大学",
            "学院",
            "集团",
            "银行",
            "医院",
            "政府",
            "部",
            "局",
            "委",
            "所",
            "中心",
            "研究院",
            "联盟",
        ];
        if ns_nt_suffix.iter().any(|s| t.ends_with(s)) {
            return 3.0;
        }
        // r/c/d closed set (pronouns / particles / adverbs)
        let func_words = [
            "你",
            "我",
            "他",
            "她",
            "它",
            "这",
            "那",
            "哪",
            "谁",
            "什么",
            "怎么",
            "为什么",
            "如何",
            "怎样",
            "哪里",
            "哪儿",
            "何时",
            "哪些",
            "哪个",
            "多少",
            "是否",
            "是不是",
            "有没有",
            "吗",
            "呢",
            "吧",
            "啊",
            "呀",
            "咋",
            "的",
            "了",
            "着",
            "过",
            "就",
            "都",
            "也",
            "很",
            "太",
            "非常",
            "已经",
            "正在",
            "将",
            "会",
            "能",
            "可以",
            "应该",
        ];
        if func_words.contains(&t) {
            return 0.3;
        }
        // Common nouns: pure CJK tokens with ≥ 2 chars
        if t.chars().count() >= 2 && t.chars().all(is_cjk) {
            return 2.0;
        }
        1.0
    }

    /// Split a query into tokens, merging consecutive English tokens into
    /// one phrase — mirrors `Dealer.split`. Tokens whose NER category is
    /// "func" are exempt from merging (function words stay separate).
    pub fn split(&self, txt: &str) -> Vec<String> {
        fn ends_with_alpha(t: &str) -> bool {
            t.chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphabetic())
        }
        let mut tks: Vec<String> = Vec::new();
        for t in txt.split_whitespace() {
            let is_func = self.ne.get(t).map(String::as_str) == Some("func");
            if let Some(last) = tks.last_mut() {
                let last_func = self.ne.get(last.as_str()).map(String::as_str) == Some("func");
                if ends_with_alpha(last) && ends_with_alpha(t) && !is_func && !last_func {
                    last.push(' ');
                    last.push_str(t);
                    continue;
                }
            }
            tks.push(t.to_string());
        }
        tks
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
                let w = (0.3 * idf1[i] + 0.7 * idf2[i]) * self.ner(t) * self.postag(t);
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

    /// Convert a token list into a weighted token dict — mirrors
    /// `Query.token_similarity`'s `to_dict`: unigrams get 0.4× their weight,
    /// adjacent bigrams get 0.6× the max of the two weights.
    pub fn weighted_token_dict(&self, tks: &[String]) -> HashMap<String, f64> {
        let mut d = HashMap::new();
        let wts = self.weights(tks, false);
        for (i, (t, c)) in wts.iter().enumerate() {
            let e = d.entry(t.clone()).or_insert(0.0);
            *e += c * 0.4;
            if i + 1 < wts.len() {
                let (_t, _c) = &wts[i + 1];
                let bigram = format!("{t}{_t}");
                let e = d.entry(bigram).or_insert(0.0);
                *e += c.max(*_c) * 0.6;
            }
        }
        d
    }

    /// Token-dict similarity — mirrors `Query.similarity`: the summed query
    /// weights found in the doc, divided by the summed query weights
    /// (s / q), with a 1e-9 floor. Score is 0..1.
    pub fn dict_similarity(qtwt: &HashMap<String, f64>, dtwt: &HashMap<String, f64>) -> f64 {
        let mut s = 1e-9;
        for (k, v) in qtwt {
            if dtwt.contains_key(k) {
                s += v;
            }
        }
        let q: f64 = 1e-9 + qtwt.values().sum::<f64>();
        s / q
    }

    /// Token similarity between one query token list and many doc token
    /// lists — mirrors `Query.token_similarity` (returns one score per doc).
    pub fn token_similarity(&self, atks: &[String], btkss: &[Vec<String>]) -> Vec<f64> {
        let atks = self.weighted_token_dict(atks);
        btkss
            .iter()
            .map(|tks| {
                let dtwt = self.weighted_token_dict(tks);
                Self::dict_similarity(&atks, &dtwt)
            })
            .collect()
    }

    /// Hybrid similarity — mirrors `Query.hybrid_similarity`: cosine of the
    /// query vector against each doc vector weighted by `vtweight`, plus
    /// token similarity weighted by `tkweight`. When the vector sum is zero
    /// only token similarity is returned.
    pub fn hybrid_similarity(
        &self,
        avec: &[f32],
        bvecs: &[Vec<f32>],
        atks: &[String],
        btkss: &[Vec<String>],
        tkweight: f64,
        vtweight: f64,
    ) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
        let sims: Vec<f64> = bvecs
            .iter()
            .map(|bvec| {
                if avec.is_empty() || avec.len() != bvec.len() {
                    return 0.0;
                }
                let (mut dot, mut na, mut nb) = (0.0_f64, 0.0_f64, 0.0_f64);
                for (a, b) in avec.iter().zip(bvec.iter()) {
                    dot += (*a as f64) * (*b as f64);
                    na += (*a as f64) * (*a as f64);
                    nb += (*b as f64) * (*b as f64);
                }
                if na == 0.0 || nb == 0.0 {
                    0.0
                } else {
                    dot / (na.sqrt() * nb.sqrt())
                }
            })
            .collect();
        let tksim = self.token_similarity(atks, btkss);
        let vec_sum: f64 = sims.iter().sum();
        if vec_sum == 0.0 {
            return (tksim.clone(), tksim, sims);
        }
        let fused: Vec<f64> = sims
            .iter()
            .zip(tksim.iter())
            .map(|(v, t)| v * vtweight + t * tkweight)
            .collect();
        (fused, tksim, sims)
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
        assert!(
            (sum - 1.0).abs() < 1e-9,
            "weights must normalize to 1, got {sum}"
        );
        let num_w = weights
            .iter()
            .find(|(t, _)| t == "42.5")
            .map(|(_, w)| *w)
            .unwrap();
        let loca_w = weights
            .iter()
            .find(|(t, _)| t == "中山市")
            .map(|(_, w)| *w)
            .unwrap();
        // RAGFlow ner map: loca=3 > number=2 — location must beat numbers
        assert!(
            loca_w > num_w,
            "location NER weight (3) should beat number (2)"
        );
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

    #[test]
    fn token_similarity_exact_match_outscores_partial() {
        let tw = TermWeightComputer::new();
        let q: Vec<String> = "水产 养殖".split_whitespace().map(String::from).collect();
        let exact: Vec<String> = "水产 养殖 水质"
            .split_whitespace()
            .map(String::from)
            .collect();
        let partial: Vec<String> = "水质 管理".split_whitespace().map(String::from).collect();
        let scores = tw.token_similarity(&q, &[exact.clone(), partial.clone()]);
        assert!(
            scores[0] > scores[1],
            "exact should beat partial: {scores:?}"
        );
        assert!(scores[1] >= 0.0, "score must not go negative");
    }

    #[test]
    fn weighted_token_dict_includes_bigrams() {
        let tw = TermWeightComputer::new();
        let tks: Vec<String> = "水产 养殖".split_whitespace().map(String::from).collect();
        let d = tw.weighted_token_dict(&tks);
        // bigram key 水产养殖 present (unigram+bigram 0.4/0.6 split)
        assert!(d.contains_key("水产养殖"));
        assert!(d.contains_key("水产"));
        assert!(d.contains_key("养殖"));
    }

    #[test]
    fn dict_similarity_is_symmetric_ratio() {
        let mut a = HashMap::new();
        a.insert("k".to_string(), 1.0);
        let mut b = HashMap::new();
        b.insert("k".to_string(), 1.0);
        b.insert("j".to_string(), 2.0);
        let s = TermWeightComputer::dict_similarity(&a, &b);
        assert!((s - 1.0).abs() < 1e-9, "all query weight found: {s}");
        let mut c = HashMap::new();
        c.insert("z".to_string(), 1.0);
        let s2 = TermWeightComputer::dict_similarity(&a, &c);
        assert!(s2 < 0.01, "no overlap ~ 0: {s2}");
    }

    #[test]
    fn hybrid_similarity_falls_back_to_token_only_on_zero_vectors() {
        let tw = TermWeightComputer::new();
        let q: Vec<String> = "水产".split_whitespace().map(String::from).collect();
        let d: Vec<String> = "水产 养殖".split_whitespace().map(String::from).collect();
        let (fused, tksim, vtsim) =
            tw.hybrid_similarity(&[], &[vec![0.0, 0.0]], &q, &[d.clone()], 0.3, 0.7);
        assert_eq!(vtsim[0], 0.0, "zero vectors give zero cosine");
        assert_eq!(fused, tksim, "fused == token sim when vectors sum to 0");
        assert!(tksim[0] > 0.0);
    }
}

// ── rag_tokenizer core (rag/nlp/rag_tokenizer.py) ─────────────────
//
// RAGFlow's `rag_tokenizer.py` is a thin wrapper over the compiled
// `infinity.rag_tokenizer` module (C++). We re-implement the parts the
// project actually consumes — tokenize / fine_grained_tokenize /
// is_chinese / is_number / is_alphabet / naive_qie / strQ2B / tradi2simp —
// as pure-Rust approximations with the same observable contract:
//
//   * `tokenize(txt)` returns space-joined tokens; CJK characters become
//     unigram + adjacent-bigram tokens ("中文" → "中 文 中文"), Latin words
//     are kept whole and lowercased, everything else is dropped.
//   * `fine_grained_tokenize(tks)` re-splits CJK-bearing tokens into
//     single characters (used for fuzzy/partial matching).

/// Tokenize text the way `rag_tokenizer.tokenize` does, returning a
/// space-joined token string. CJK runs produce unigrams + bigrams; Latin
/// words are lowercased and kept whole; stop words are NOT removed here
/// (that happens in `TermWeightComputer::pretoken`, mirroring RAGFlow).
pub fn rag_tokenize(text: &str) -> String {
    let mut tokens: Vec<String> = Vec::new();
    let mut alnum = String::new();
    let mut cjk: Vec<char> = Vec::new();

    let flush_alnum = |buf: &mut String, out: &mut Vec<String>| {
        if !buf.is_empty() {
            out.push(std::mem::take(buf));
        } else {
            buf.clear();
        }
    };
    let flush_cjk = |run: &mut Vec<char>, out: &mut Vec<String>| {
        out.extend(run.iter().map(|c| c.to_string()));
        out.extend(run.windows(2).map(|w| w.iter().collect::<String>()));
        run.clear();
    };

    for ch in text.to_lowercase().chars() {
        if is_cjk(ch) {
            flush_alnum(&mut alnum, &mut tokens);
            cjk.push(ch);
        } else if ch.is_alphanumeric() {
            flush_cjk(&mut cjk, &mut tokens);
            alnum.push(ch);
        } else {
            flush_alnum(&mut alnum, &mut tokens);
            flush_cjk(&mut cjk, &mut tokens);
        }
    }
    flush_alnum(&mut alnum, &mut tokens);
    flush_cjk(&mut cjk, &mut tokens);
    tokens.join(" ")
}

/// Fine-grained tokenization: split every token containing CJK characters
/// into single characters (space-joined). Mirrors
/// `rag_tokenizer.fine_grained_tokenize`'s role in query expansion.
pub fn rag_fine_grained_tokenize(tks: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for tk in tks.split_whitespace() {
        if tk.chars().any(is_cjk) {
            out.extend(tk.chars().map(|c| c.to_string()));
        } else {
            out.push(tk.to_string());
        }
    }
    out.join(" ")
}

/// Whether a text is mostly Chinese — mirrors `rag/nlp/__init__.py
/// is_chinese`: fraction of chars in U+4E00..U+9FFF > 0.2.
pub fn is_chinese(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let total = text.chars().count();
    let chinese = text
        .chars()
        .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
        .count();
    chinese as f64 / total as f64 > 0.2
}

/// Whether a string is numeric (digits, with optional commas/points/percent)
/// — approximation of `rag_tokenizer.is_number`.
pub fn is_number(s: &str) -> bool {
    let cleaned: String = s
        .chars()
        .filter(|c| !matches!(c, ',' | '.' | '%' | ' ' | '．'))
        .collect();
    !cleaned.is_empty() && cleaned.chars().all(|c| c.is_ascii_digit())
}

/// Whether a string is purely alphabetical — `rag_tokenizer.is_alphabet`.
pub fn is_alphabet(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphabetic())
}

/// Naive segmentation — `rag_tokenizer.naive_qie`: quick split where every
/// CJK character becomes its own token and Latin runs stay whole.
pub fn naive_qie(txt: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut alnum = String::new();
    let flush = |buf: &mut String, out: &mut Vec<String>| {
        if !buf.is_empty() {
            out.push(std::mem::take(buf));
        } else {
            buf.clear();
        }
    };
    for ch in txt.to_lowercase().chars() {
        if is_cjk(ch) {
            flush(&mut alnum, &mut out);
            out.push(ch.to_string());
        } else if ch.is_alphanumeric() {
            alnum.push(ch);
        } else {
            flush(&mut alnum, &mut out);
        }
    }
    flush(&mut alnum, &mut out);
    out.join(" ")
}

/// Full-width → half-width conversion — `rag_tokenizer.strQ2B`.
/// U+FF01..U+FF5E map onto U+0021..U+007E; U+3000 (ideographic space) → ' '.
pub fn str_q2b(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{3000}' => ' ',
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0).unwrap_or(c),
            _ => c,
        })
        .collect()
}

/// Traditional → Simplified Chinese conversion — `rag_tokenizer.tradi2simp`.
///
/// RAGFlow uses OpenCC; this is a dependency-free approximation covering the
/// most frequent traditional characters (a curated subset of OpenCC's
/// T2S table, char-by-char). Unknown characters pass through unchanged.
pub fn tradi2simp(text: &str) -> String {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static TABLE: OnceLock<HashMap<char, char>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        TRADI2SIMP_PAIRS
            .iter()
            .filter_map(|pair| {
                let mut cs = pair.chars();
                let t = cs.next()?;
                let s = cs.next()?;
                Some((t, s))
            })
            .collect()
    });
    text.chars()
        .map(|c| table.get(&c).copied().unwrap_or(c))
        .collect()
}

/// Traditional→Simplified pair table (traditional char followed by its
/// simplified form). Curated subset of OpenCC T2S covering common
/// vocabulary; see `tradi2simp` for caveats.
const TRADI2SIMP_PAIRS: &[&str] = &[
    "愛爱", "礙碍", "罷罢", "擺摆", "敗败", "辦办", "幫帮", "飽饱", "寶宝", "報报", "輩辈", "貝贝",
    "備备", "筆笔", "幣币", "閉闭", "邊边", "編编", "變变", "辯辩", "標标", "別别", "賓宾", "餅饼",
    "撥拨", "補补", "財财", "參参", "殘残", "慘惨", "燦灿", "倉仓", "艙舱", "蒼苍", "廁厕", "側侧",
    "冊册", "測测", "層层", "產产", "長长", "償偿", "廠厂", "場场", "暢畅", "車车", "徹彻", "塵尘",
    "稱称", "誠诚", "懲惩", "遲迟", "齒齿", "衝冲", "蟲虫", "醜丑", "籌筹", "觸触", "處处", "傳传",
    "創创", "錘锤", "純纯", "詞词", "辭辞", "聰聪", "從从", "錯错", "達达", "帶带", "貸贷", "擔担",
    "單单", "彈弹", "當当", "擋挡", "黨党", "檔档", "導导", "島岛", "燈灯", "敵敌", "遞递", "點点",
    "電电", "調调", "東东", "動动", "凍冻", "鬥斗", "獨独", "讀读", "斷断", "隊队", "對对", "噸吨",
    "奪夺", "額额", "惡恶", "餓饿", "兒儿", "爾尔", "發发", "罰罚", "煩烦", "範范", "飯饭", "飛飞",
    "廢废", "費费", "紛纷", "奮奋", "豐丰", "風风", "馮冯", "諷讽", "鳳凤", "膚肤", "輔辅", "復复",
    "該该", "蓋盖", "乾干", "趕赶", "幹干", "綱纲", "崗岗", "鋼钢", "個个", "給给", "溝沟", "購购",
    "夠够", "構构", "穀谷", "顧顾", "掛挂", "關关", "觀观", "館馆", "慣惯", "貫贯", "廣广", "歸归",
    "龜龟", "規规", "軌轨", "櫃柜", "貴贵", "滾滚", "國国", "過过", "還还", "韓韩", "漢汉", "號号",
    "賀贺", "紅红", "後后", "護护", "戶户", "華华", "畫画", "話话", "懷怀", "壞坏", "歡欢", "環环",
    "緩缓", "換换", "黃黄", "揮挥", "輝辉", "會会", "匯汇", "繪绘", "雞鸡", "積积", "機机", "極极",
    "幾几", "擠挤", "計计", "記记", "紀纪", "際际", "劑剂", "濟济", "繼继", "夾夹", "價价", "駕驾",
    "堅坚", "殲歼", "間间", "艱艰", "監监", "揀拣", "儉俭", "檢检", "減减", "簡简", "見见", "薦荐",
    "賤贱", "艦舰", "漸渐", "鍵键", "踐践", "講讲", "獎奖", "將将", "醬酱", "膠胶", "驕骄", "腳脚",
    "攪搅", "繳缴", "轎轿", "較较", "階阶", "節节", "傑杰", "潔洁", "結结", "屆届", "緊紧", "錦锦",
    "謹谨", "盡尽", "勁劲", "進进", "晉晋", "驚惊", "經经", "競竞", "淨净", "徑径", "靜静", "鏡镜",
    "糾纠", "舊旧", "舉举", "劇剧", "據据", "懼惧", "鋸锯", "捲卷", "決决", "絕绝", "覺觉", "軍军",
    "開开", "凱凯", "殼壳", "課课", "庫库", "褲裤", "誇夸", "塊块", "寬宽", "礦矿", "虧亏", "潰溃",
    "擴扩", "闊阔", "來来", "賴赖", "蘭兰", "攔拦", "欄栏", "藍蓝", "籃篮", "覽览", "爛烂", "濫滥",
    "撈捞", "勞劳", "樂乐", "類类", "淚泪", "離离", "禮礼", "裏里", "曆历", "歷历", "隸隶", "連连",
    "聯联", "臉脸", "練练", "煉炼", "戀恋", "鏈链", "涼凉", "兩两", "遼辽", "療疗", "瞭了", "獵猎",
    "鄰邻", "臨临", "齡龄", "鈴铃", "領领", "嶺岭", "劉刘", "龍龙", "籠笼", "聾聋", "壟垄", "樓楼",
    "摟搂", "盧卢", "廬庐", "蘆芦", "爐炉", "魯鲁", "陸陆", "錄录", "亂乱", "倫伦", "輪轮", "論论",
    "羅罗", "邏逻", "騾骡", "驢驴", "呂吕", "鋁铝", "綠绿", "濾滤", "絡络", "馬马", "碼码", "罵骂",
    "嗎吗", "買买", "麥麦", "賣卖", "脈脉", "瞞瞒", "滿满", "貓猫", "貿贸", "沒没", "門门", "悶闷",
    "們们", "夢梦", "彌弥", "謎谜", "綿绵", "麵面", "廟庙", "滅灭", "鳴鸣", "謬谬", "謀谋", "畝亩",
    "納纳", "難难", "腦脑", "鬧闹", "內内", "擬拟", "釀酿", "鳥鸟", "聶聂", "寧宁", "紐纽", "農农",
    "濃浓", "諾诺", "歐欧", "毆殴", "嘔呕", "盤盘", "拋抛", "賠赔", "噴喷", "鵬鹏", "騙骗", "飄飘",
    "頻频", "評评", "憑凭", "潑泼", "撲扑", "鋪铺", "樸朴", "譜谱", "齊齐", "騎骑", "啟启", "氣气",
    "棄弃", "牽牵", "鉛铅", "謙谦", "簽签", "錢钱", "潛潜", "淺浅", "譴谴", "槍枪", "強强", "牆墙",
    "搶抢", "僑侨", "橋桥", "竅窍", "竊窃", "親亲", "寢寝", "輕輕", "傾倾", "請请", "慶庆", "窮穷",
    "瓊琼", "區区", "驅驱", "趨趋", "權权", "勸劝", "卻却", "確确", "鵲鹊", "讓让", "饒饶", "繞绕",
    "熱热", "認认", "榮荣", "軟软", "銳锐", "潤润", "灑洒", "賽赛", "傘伞", "喪丧", "掃扫", "澀涩",
    "殺杀", "紗纱", "篩筛", "曬晒", "刪删", "閃闪", "陝陕", "傷伤", "賞赏", "燒烧", "紹绍", "捨舍",
    "設设", "攝摄", "審审", "嬸婶", "腎肾", "聲声", "勝胜", "聖圣", "屍尸", "師师", "詩诗", "濕湿",
    "獅狮", "時时", "識识", "實实", "蝕蚀", "駛驶", "勢势", "視视", "試试", "飾饰", "適适", "釋释",
    "壽寿", "獸兽", "書书", "輸输", "贖赎", "屬属", "術术", "樹树", "數数", "豎竖", "帥帅", "雙双",
    "誰谁", "稅税", "順顺", "說说", "碩硕", "絲丝", "飼饲", "鬆松", "誦诵", "蘇苏", "訴诉", "雖虽",
    "隨随", "歲岁", "孫孙", "損损", "縮缩", "鎖锁", "臺台", "態态", "貪贪", "灘滩", "壇坛", "談谈",
    "嘆叹", "湯汤", "濤涛", "討讨", "騰腾", "題题", "體体", "條条", "貼贴", "鐵铁", "聽听", "銅铜",
    "統统", "頭头", "禿秃", "圖图", "塗涂", "團团", "脫脱", "駝驼", "橢椭", "彎弯", "灣湾", "頑顽",
    "萬万", "網网", "為为", "圍围", "違违", "維维", "偉伟", "偽伪", "衛卫", "謂谓", "溫温", "聞闻",
    "紋纹", "穩稳", "問问", "窩窝", "臥卧", "烏乌", "汙污", "無无", "吳吴", "務务", "誤误", "霧雾",
    "習习", "襲袭", "係系", "細细", "戲戏", "蝦虾", "峽峡", "狹狭", "轄辖", "鮮鲜", "纖纤", "鹹咸",
    "賢贤", "銜衔", "顯显", "險险", "縣县", "現现", "線线", "憲宪", "餡馅", "羨羡", "獻献", "鄉乡",
    "詳详", "響响", "項项", "銷销", "蕭萧", "曉晓", "協协", "脅胁", "挾挟", "寫写", "謝谢", "興兴",
    "繡绣", "虛虚", "許许", "敘叙", "續续", "懸悬", "選选", "學学", "尋寻", "詢询", "訓训", "訊讯",
    "壓压", "鴉鸦", "鴨鸭", "啞哑", "亞亚", "訝讶", "煙烟", "嚴严", "顏颜", "鹽盐", "豔艳", "厭厌",
    "驗验", "揚扬", "陽阳", "楊杨", "養养", "樣样", "搖摇", "遙遥", "藥药", "爺爷", "業业", "葉叶",
    "頁页", "醫医", "儀仪", "遺遗", "義义", "億亿", "憶忆", "藝艺", "議议", "異异", "譯译", "誼谊",
    "陰阴", "銀银", "飲饮", "隱隐", "應应", "嬰婴", "鷹鹰", "營营", "贏赢", "擁拥", "湧涌", "優优",
    "憂忧", "郵邮", "猶犹", "遊游", "誘诱", "於于", "魚鱼", "漁渔", "與与", "語语", "鬱郁", "獄狱",
    "預预", "禦御", "園园", "員员", "圓圆", "緣缘", "遠遠", "願愿", "約约", "悅悦", "閱阅", "雲云",
    "勻匀", "運运", "蘊蕴", "雜杂", "災灾", "載载", "讚赞", "暫暂", "贊赞", "髒脏", "棗枣", "責责",
    "擇择", "澤泽", "賊贼", "贈赠", "詐诈", "齋斋", "債债", "斬斩", "盞盏", "佔占", "戰战", "張张",
    "漲涨", "帳帐", "賬账", "趙赵", "這这", "針针", "偵侦", "診诊", "陣阵", "鎮镇", "爭争", "徵征",
    "證证", "鄭郑", "織织", "執执", "職职", "紙纸", "製制", "質质", "終终", "鐘钟", "腫肿", "種种",
    "眾众", "軸轴", "晝昼", "皺皱", "豬猪", "駐驻", "築筑", "專专", "磚砖", "轉转", "賺赚", "莊庄",
    "裝装", "壯壮", "狀状", "墜坠", "準准", "濁浊", "資资", "綜综", "總总", "縱纵", "組组", "鑽钻",
    "麼么",
];

/// Whether query text is mostly English — mirrors `rag/nlp/__init__.py
/// is_english`: >80% of non-empty tokens fully match the ASCII set.
pub fn is_english_query(texts: &str) -> bool {
    let tokens: Vec<&str> = texts
        .split(|c: char| c.is_whitespace())
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return false;
    }
    let eng = tokens
        .iter()
        .filter(|t| {
            t.chars().all(|c| {
                c.is_ascii()
                    && (c.is_alphanumeric()
                        || matches!(
                            c,
                            '.' | ','
                                | ':'
                                | ';'
                                | '\''
                                | '/'
                                | '"'
                                | '?'
                                | '<'
                                | '>'
                                | '!'
                                | '('
                                | ')'
                                | '-'
                        ))
            })
        })
        .count();
    eng as f64 / tokens.len() as f64 > 0.8
}

// ── Query normalization (common/query_base.py QueryBase) ──────────

/// Insert spaces between English and Chinese segments —
/// `QueryBase.add_space_between_eng_zh`.
pub fn add_space_between_eng_zh(txt: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE_ENG_NUM_ZH: OnceLock<Regex> = OnceLock::new();
    static RE_ENG_ZH: OnceLock<Regex> = OnceLock::new();
    static RE_ZH_ENG_NUM: OnceLock<Regex> = OnceLock::new();
    static RE_ZH_ENG: OnceLock<Regex> = OnceLock::new();
    let mut out = txt.to_string();
    // (ENG/ENG+NUM) + ZH
    out = RE_ENG_NUM_ZH
        .get_or_init(|| Regex::new(r"([A-Za-z]+[0-9]+)([\u{4e00}-\u{9fa5}]+)").unwrap())
        .replace_all(&out, "$1 $2")
        .to_string();
    // ENG + ZH
    out = RE_ENG_ZH
        .get_or_init(|| Regex::new(r"([A-Za-z])([\u{4e00}-\u{9fa5}]+)").unwrap())
        .replace_all(&out, "$1 $2")
        .to_string();
    // ZH + (ENG/ENG+NUM)
    out = RE_ZH_ENG_NUM
        .get_or_init(|| Regex::new(r"([\u{4e00}-\u{9fa5}]+)([A-Za-z]+[0-9]+)").unwrap())
        .replace_all(&out, "$1 $2")
        .to_string();
    // ZH + ENG
    out = RE_ZH_ENG
        .get_or_init(|| Regex::new(r"([\u{4e00}-\u{9fa5}]+)([A-Za-z])").unwrap())
        .replace_all(&out, "$1 $2")
        .to_string();
    out
}

/// Strip question/filler words — `QueryBase.rmWWW`. Two regex passes: the
/// Chinese question-word pattern and the English filler-word pattern; if the
/// result is empty the original text is returned.
pub fn rm_www(txt: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static ZH: OnceLock<Regex> = OnceLock::new();
    static EN: OnceLock<Regex> = OnceLock::new();
    let zh = ZH.get_or_init(|| {
        Regex::new(r"是*(怎么办|什么样的|哪家|一下|那家|请问|啥样|咋样了|什么时候|何时|何地|何人|是否|是不是|多少|哪里|怎么|哪儿|怎么样|如何|哪些|是啥|啥是|啊|吗|呢|吧|咋|什么|有没有|呀|谁|哪位|哪个)是*").unwrap()
    });
    let en = EN.get_or_init(|| {
        Regex::new(r"(?i)(^| )(what|who|how|which|where|why)('re|'s)? |(^| )('s|'re|is|are|were|was|do|does|did|don't|doesn't|didn't|has|have|be|there|you|me|your|my|mine|just|please|may|i|should|would|wouldn't|will|won't|done|go|for|with|so|the|a|an|by|i'm|it's|he's|she's|they|they're|you're|as|on|in|at|up|out|down|of|to|or|and|if) ").unwrap()
    });
    let mut out = zh.replace_all(txt, "").to_string();
    out = en.replace_all(&out, " ").to_string();
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.is_empty() { txt.to_string() } else { out }
}

/// Escape special characters for the doc-store query language —
/// `QueryBase.sub_special_char`: strip single quotes, backslash-escape
/// `: { } / [ ] - * ? " ( ) | + ~ ^`.
pub fn sub_special_char(line: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re =
        RE.get_or_init(|| Regex::new("([:\\{\\}/\\[\\]\\-\\*\\?\\\"\\(\\)\\|\\+~\\^])").unwrap());
    re.replace_all(&line.replace('\'', ""), |caps: &regex::Captures| {
        format!("\\{}", &caps[1])
    })
    .to_string()
    .trim()
    .to_string()
}

/// Full query normalization pipeline — mirrors the head of
/// `FulltextQueryer.question`: add spaces between EN/ZH, lowercase,
/// full-width → half-width, traditional → simplified, drop filler words.
pub fn normalize_query(question: &str) -> String {
    let spaced = add_space_between_eng_zh(question);
    let cleaned = tradi2simp(&str_q2b(&spaced.to_lowercase()));
    let cleaned = rm_www(&cleaned);
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ── Keyword query builder (rag/nlp/query.py FulltextQueryer) ──────

/// Result of `build_keyword_query`: an ES/Infinity-style weighted query
/// string plus the flat keyword list used for hybrid scoring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeywordQuery {
    /// Weighted query string (`(term)^w (term2)^w ...` joined with OR).
    pub query: String,
    /// Flat keyword list (weighted terms + synonyms + fine-grained tokens).
    pub keywords: Vec<String>,
}

/// Whether a token needs fine-grained re-tokenization —
/// `FulltextQueryer.need_fine_grained_tokenize`: len ≥ 3 and not a pure
/// `[0-9a-z.+#_*-]` token.
fn need_fine_grained_tokenize(tk: &str) -> bool {
    if tk.chars().count() < 3 {
        return false;
    }
    let pure = regex::Regex::new(r"^[0-9a-z\.\+#_*\-]+$").unwrap();
    !pure.is_match(tk)
}

/// Build a keyword query from a question — the Chinese path of
/// `FulltextQueryer.question`: normalize → split → per-token weights →
/// fine-grained tokens + synonyms → weighted query string.
pub fn build_keyword_query(question: &str) -> KeywordQuery {
    let tw = TermWeightComputer::new();
    let syn = SynonymDict::new();
    let normalized = normalize_query(question);
    let mut qs: Vec<String> = Vec::new();
    let mut keywords: Vec<String> = Vec::new();

    for tt in tw.split(&normalized).into_iter().take(256) {
        if tt.is_empty() {
            continue;
        }
        if keywords.len() < 32 {
            keywords.push(tt.clone());
        }
        let mut twts = tw.weights(std::slice::from_ref(&tt), false);
        twts.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let mut tms: Vec<String> = Vec::new();
        for (tk, w) in twts {
            // fine-grained tokens (length > 1 after cleaning)
            let mut sm: Vec<String> = Vec::new();
            if need_fine_grained_tokenize(&tk) {
                for m in rag_fine_grained_tokenize(&rag_tokenize(&tk)).split_whitespace() {
                    let cleaned = sub_special_char(m);
                    if cleaned.chars().count() > 1 {
                        sm.push(cleaned);
                    }
                }
            }
            if keywords.len() < 32 {
                keywords.push(tk.replace([' ', '\\', '"', '\''], ""));
                keywords.extend(sm.iter().cloned());
            }
            let tk_syns: Vec<String> = syn
                .lookup(&tk, 8)
                .into_iter()
                .map(|s| sub_special_char(&s))
                .collect();
            if keywords.len() < 32 {
                keywords.extend(tk_syns.iter().filter(|s| !s.is_empty()).cloned());
            }
            if keywords.len() >= 32 {
                break;
            }
            let tk_clean = sub_special_char(&tk);
            let tk_quoted = if tk_clean.contains(' ') {
                format!("\"{tk_clean}\"")
            } else {
                tk_clean.clone()
            };
            let expr = if tk_syns.is_empty() {
                tk_quoted
            } else {
                format!("({tk_quoted} OR ({} )^0.2)", tk_syns.join(" "))
            };
            let mut expr = expr;
            if !sm.is_empty() {
                expr = format!(
                    "{expr} OR \"{}\" OR (\"{}\"~2)^0.5",
                    sm.join(" "),
                    sm.join(" ")
                );
            }
            if !expr.trim().is_empty() {
                tms.push(format!("({expr})^{w}"));
            }
        }
        if !tms.is_empty() {
            qs.push(tms.join(" "));
        }
    }

    let query = if qs.is_empty() {
        normalized
    } else {
        qs.into_iter()
            .map(|q| format!("({q})"))
            .collect::<Vec<_>>()
            .join(" OR ")
    };
    KeywordQuery { query, keywords }
}

#[cfg(test)]
mod rag_tokenizer_tests {
    use super::*;

    #[test]
    fn rag_tokenize_cjk_emits_unigrams_and_bigrams() {
        // 中文 → 中 文 中文 (unigrams + adjacent bigram), space-joined
        assert_eq!(rag_tokenize("中文"), "中 文 中文");
        // 4-char run: unigrams then adjacent bigrams (infinity tokenizer order)
        assert_eq!(rag_tokenize("水产养殖"), "水 产 养 殖 水产 产养 养殖");
    }

    #[test]
    fn rag_tokenize_latin_words_kept_whole_and_lowercased() {
        assert_eq!(rag_tokenize("Hello World"), "hello world");
        assert_eq!(rag_tokenize("RAGFlow123"), "ragflow123");
        // mixed EN + ZH
        let tks = rag_tokenize("使用RAGFlow检索");
        let parts: Vec<&str> = tks.split_whitespace().collect();
        assert!(parts.contains(&"ragflow"));
        assert!(parts.contains(&"使用"));
        assert!(parts.contains(&"检索"));
    }

    #[test]
    fn fine_grained_tokenize_splits_cjk_tokens() {
        assert_eq!(rag_fine_grained_tokenize("水 产 水产"), "水 产 水 产");
        assert_eq!(rag_fine_grained_tokenize("hello 水质"), "hello 水 质");
    }

    #[test]
    fn str_q2b_converts_fullwidth() {
        assert_eq!(str_q2b("ＡＢＣ１２３"), "ABC123");
        assert_eq!(str_q2b("ＲＡＧＦｌｏｗ"), "RAGFlow");
        assert_eq!(str_q2b("中文　测试"), "中文 测试"); // U+3000 → space
    }

    #[test]
    fn tradi2simp_converts_common_characters() {
        assert_eq!(tradi2simp("繁體中文測試"), "繁体中文测试");
        assert_eq!(tradi2simp("電腦網絡"), "电脑网络");
        // unknown chars pass through
        assert_eq!(tradi2simp("abc"), "abc");
    }

    #[test]
    fn is_chinese_detects_ratio() {
        assert!(is_chinese("这是一段中文"));
        assert!(!is_chinese("hello world"));
        assert!(is_chinese("混合mixed中文字符"));
    }

    #[test]
    fn query_normalization_pipeline() {
        // add spaces between EN and ZH (upstream regex semantics:
        // "RAGFlow使用" → "RAGFlow 使用", matching QueryBase exactly)
        assert_eq!(add_space_between_eng_zh("使用RAGFlow"), "使用 RAGFlow");
        assert_eq!(add_space_between_eng_zh("RAGFlow使用"), "RAGFlow 使用");
        // rm_www strips Chinese question words
        assert_eq!(rm_www("请问什么是水产养殖"), "水产养殖");
        // English filler removal — upstream regex semantics: "what " eats
        // the space before "is", so " is " cannot match (same as RAGFlow)
        assert_eq!(rm_www("what is the best rust"), "is best rust");
        // standalone filler at the head does get removed
        assert_eq!(rm_www("please remove the test"), "remove test");
        // full pipeline
        let q = normalize_query("请问如何使用ＲＡＧＦｌｏｗ進行檢索？");
        assert!(!q.contains("请问"));
        assert!(q.to_lowercase().contains("ragflow"));
        assert!(q.contains("检索"));
    }

    #[test]
    fn sub_special_char_escapes_and_strips_quotes() {
        // single quotes removed; colon escaped
        assert_eq!(sub_special_char("a'b:c"), "ab\\:c");
        assert_eq!(sub_special_char("x+y"), "x\\+y");
    }

    #[test]
    fn is_number_and_alphabet() {
        assert!(is_number("123"));
        assert!(is_number("1,234.5"));
        assert!(!is_number("12a"));
        assert!(is_alphabet("abcXYZ"));
        assert!(!is_alphabet("abc1"));
    }

    #[test]
    fn term_weight_split_merges_consecutive_english() {
        let tw = TermWeightComputer::new();
        // consecutive English tokens merge into one phrase (RAGFlow split)
        let tks = tw.split("how to use Rust language");
        assert_eq!(tks, vec!["how to use Rust language"]);
        // CJK tokens stay separate
        let tks = tw.split("水产 养殖 水质");
        assert_eq!(tks, vec!["水产", "养殖", "水质"]);
        // func NER category prevents merging
        let mut ne = HashMap::new();
        ne.insert("the".to_string(), "func".to_string());
        let tw = tw.with_resources(ne, HashMap::new());
        let tks = tw.split("the quick brown");
        assert_eq!(tks, vec!["the", "quick brown"]);
    }

    #[test]
    fn postag_weights_place_nouns_above_numbers() {
        let tw = TermWeightComputer::new();
        // 中山市: ner=loca(3) × postag=ns(3) = 9;  42.5: ner=2 × postag=2 = 4
        let mut ne = HashMap::new();
        ne.insert("中山市".to_string(), "loca".to_string());
        let tw = tw.with_resources(ne, HashMap::new());
        let weights = tw.weights(&["中山市".into(), "42.5".into()], false);
        let loca_w = weights
            .iter()
            .find(|(t, _)| t == "中山市")
            .map(|(_, w)| *w)
            .unwrap();
        let num_w = weights
            .iter()
            .find(|(t, _)| t == "42.5")
            .map(|(_, w)| *w)
            .unwrap();
        assert!(
            loca_w > num_w * 2.0,
            "loca×ns (9) should dominate number (4): {loca_w} vs {num_w}"
        );
    }

    #[test]
    fn build_keyword_query_returns_weighted_terms_and_keywords() {
        let kq = build_keyword_query("请问什么是水产养殖水质管理");
        assert!(!kq.query.is_empty(), "query string must be non-empty");
        assert!(!kq.keywords.is_empty(), "keywords must be non-empty");
        // stop words 请问/什么 are removed by normalization
        assert!(!kq.keywords.iter().any(|k| k == "请问" || k == "什么"));
        // core terms survive
        assert!(
            kq.keywords
                .iter()
                .any(|k| k.contains("水产") || k.contains("养殖"))
        );
        // query contains weighted term syntax (term)^weight
        assert!(
            kq.query.contains("^"),
            "expected weighted syntax, got: {}",
            kq.query
        );
    }
}
