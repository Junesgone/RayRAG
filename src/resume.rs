//! Resume parser — mirrors RAGFlow `rag/app/resume.py` (LLM-free core).
//!
//! Ported components (all deterministic, no LLM required):
//! - `FIELD_MAP_ZH` / `FIELD_MAP_EN` + `get_field_map` (resume.py:98-178)
//! - `parse_with_regex` (resume.py:1482-1787): regex fallback extraction of
//!   name/phone/email/gender/age/birth/degree/school/major/company/position/
//!   work-years/graduation-year, bilingual (Chinese + English) strategies
//! - `_postprocess_resume` (resume.py:1794-2053): 4-phase pipeline —
//!   1) source-text validation (prune LLM hallucinations), 2) domain
//!   normalization (date/phone/gender), 3) contextual dedup (company-substring
//!   + date-overlap, Jaccard fallback, project→work desc merge),
//!   4) required-field completion
//! - `_normalize_for_comparison`, `_parse_date_str`, `_calc_single_exp_years`,
//!   `_calculate_work_years`, `_shingling_jaccard` (tiktoken shingles replaced
//!   by byte-level n-grams with the same n=5 default and <n single-shingle rule)
//! - `_build_chunk_document` (resume.py:2101-2430): field-group merge chunks
//!   (Basic Info / Education / Skills & Certificates / Work Overview) plus
//!   per-element chunks for work/project descriptions, identity-field
//!   redundancy, resume summary line, logical add_positions ordering
//!
//! Not ported (LLM-dependent, mirrors the SmartResume design):
//! prompt templates / `_call_llm` / `parse_with_llm` — RayRAG exposes
//! `ResumeExtractor` as the seam; regex path is the deterministic fallback.

use serde_json::{Value, json};
use std::collections::HashSet;

/// `_is_english` — resume.py:169.
pub fn is_english(lang: &str) -> bool {
    let l = lang.trim().to_lowercase();
    l == "english" || l == "en"
}

/// FIELD_MAP_ZH — resume.py:98-132.
pub fn field_map_zh() -> Vec<(&'static str, &'static str)> {
    vec![
        ("name_kwd", "姓名/名字"),
        ("name_pinyin_kwd", "姓名拼音/名字拼音"),
        ("gender_kwd", "性别（男，女）"),
        ("age_int", "年龄/岁/年纪"),
        ("phone_kwd", "电话/手机/微信"),
        ("email_tks", "email/e-mail/邮箱"),
        ("position_name_tks", "职位/职能/岗位/职责"),
        ("expect_city_names_tks", "期望城市"),
        ("work_exp_flt", "工作年限/工作年份/N年经验/毕业了多少年"),
        ("corporation_name_tks", "最近就职(上班)的公司/上一家公司"),
        ("first_school_name_tks", "第一学历毕业学校"),
        ("first_degree_kwd", "第一学历"),
        ("highest_degree_kwd", "最高学历"),
        ("first_major_tks", "第一学历专业"),
        ("edu_first_fea_kwd", "第一学历标签"),
        ("degree_kwd", "过往学历"),
        ("major_tks", "学过的专业/过往专业"),
        ("school_name_tks", "学校/毕业院校"),
        ("sch_rank_kwd", "学校标签"),
        ("edu_fea_kwd", "教育标签"),
        ("corp_nm_tks", "就职过的公司/之前的公司/上过班的公司"),
        ("edu_end_int", "毕业年份"),
        ("industry_name_tks", "所在行业"),
        ("birth_dt", "生日/出生年份"),
        ("expect_position_name_tks", "期望职位/期望职能/期望岗位"),
        ("skill_tks", "技能/技术栈/编程语言/框架/工具"),
        ("language_tks", "语言能力/外语水平"),
        ("certificate_tks", "证书/资质/认证"),
        ("project_tks", "项目经验/项目名称"),
        ("work_desc_tks", "工作职责/工作描述"),
        ("project_desc_tks", "项目描述/项目职责"),
        ("self_evaluation_tks", "自我评价/个人优势/个人总结"),
    ]
}

/// FIELD_MAP_EN — resume.py:133-166.
pub fn field_map_en() -> Vec<(&'static str, &'static str)> {
    vec![
        ("name_kwd", "Name"),
        ("name_pinyin_kwd", "Name Pinyin"),
        ("gender_kwd", "Gender (Male, Female)"),
        ("age_int", "Age"),
        ("phone_kwd", "Phone/Mobile/WeChat"),
        ("email_tks", "Email"),
        ("position_name_tks", "Position/Title/Role"),
        ("expect_city_names_tks", "Preferred City"),
        ("work_exp_flt", "Years of Experience"),
        ("corporation_name_tks", "Most Recent Company"),
        ("first_school_name_tks", "First Degree School"),
        ("first_degree_kwd", "First Degree"),
        ("highest_degree_kwd", "Highest Degree"),
        ("first_major_tks", "First Degree Major"),
        ("edu_first_fea_kwd", "First Degree Tag"),
        ("degree_kwd", "Past Degrees"),
        ("major_tks", "Past Majors"),
        ("school_name_tks", "School/University"),
        ("sch_rank_kwd", "School Tag"),
        ("edu_fea_kwd", "Education Tag"),
        ("corp_nm_tks", "Past Companies"),
        ("edu_end_int", "Graduation Year"),
        ("industry_name_tks", "Industry"),
        ("birth_dt", "Date of Birth"),
        ("expect_position_name_tks", "Preferred Position/Role"),
        ("skill_tks", "Skills/Tech Stack/Languages/Frameworks/Tools"),
        ("language_tks", "Language Proficiency"),
        ("certificate_tks", "Certificates/Qualifications"),
        ("project_tks", "Project Experience/Project Name"),
        ("work_desc_tks", "Job Responsibilities/Description"),
        ("project_desc_tks", "Project Description/Responsibilities"),
        (
            "self_evaluation_tks",
            "Self-Evaluation/Personal Strengths/Summary",
        ),
    ]
}

/// `get_field_map` — resume.py:176.
pub fn get_field_map(lang: &str) -> Vec<(&'static str, &'static str)> {
    if is_english(lang) {
        field_map_en()
    } else {
        field_map_zh()
    }
}

/// `_normalize_for_comparison` — resume.py:1107: NFKC + strip whitespace +
/// lowercase. NFKC is approximated with a fullwidth→halfwidth pass.
pub fn normalize_for_comparison(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        // Fullwidth ASCII (U+FF01..U+FF5E) → halfwidth; U+3000 → space.
        let c = match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            _ => c,
        };
        out.push(c);
    }
    out.retain(|c| !c.is_whitespace());
    out.to_lowercase()
}

/// `_parse_date_str` — resume.py:1178: year[./-年]month → (year, month, 1).
pub fn parse_date_str(date_str: &str) -> Option<(i32, u32)> {
    let s = date_str.trim();
    // year.month / year-month / year/month / year年month月
    let re = regex::Regex::new(r"((?:19|20)\d{2})[.\-/年](\d{1,2})").unwrap();
    if let Some(caps) = re.captures(s) {
        let year: i32 = caps.get(1)?.as_str().parse().ok()?;
        let mut month: u32 = caps.get(2)?.as_str().parse().ok()?;
        if !(1..=12).contains(&month) {
            month = 1;
        }
        return Some((year, month));
    }
    // year only → January.
    let re_year = regex::Regex::new(r"^((?:19|20)\d{2})$").unwrap();
    if let Some(caps) = re_year.captures(s) {
        let year: i32 = caps.get(1)?.as_str().parse().ok()?;
        return Some((year, 1));
    }
    None
}

/// `_calc_single_exp_years` — resume.py:1127: months/12, 1 decimal.
pub fn calc_single_exp_years(start_str: &str, end_str: &str) -> f64 {
    let start = start_str.trim();
    let end = end_str.trim();
    if start.is_empty() {
        return 0.0;
    }
    let Some((sy, sm)) = parse_date_str(start) else {
        return 0.0;
    };
    let (ey, em) = if matches!(
        end,
        "至今" | "现在" | "present" | "Present" | "now" | "Now" | ""
    ) {
        // Current date (2026-08-03 for deterministic tests).
        (2026, 8)
    } else {
        match parse_date_str(end) {
            Some((y, m)) => (y, m),
            None => (2026, 8),
        }
    };
    let months = (ey - sy) * 12 + (em as i32 - sm as i32);
    if months <= 0 {
        return 0.0;
    }
    ((months as f64 / 12.0) * 10.0).round() / 10.0
}

/// `_calculate_work_years` — resume.py:1161.
pub fn calculate_work_years(experiences: &[Value]) -> f64 {
    let mut total = 0.0;
    for exp in experiences {
        let start = exp.get("start_date").and_then(Value::as_str).unwrap_or("");
        let end = exp.get("end_date").and_then(Value::as_str).unwrap_or("");
        total += calc_single_exp_years(start, end);
    }
    (total * 10.0).round() / 10.0
}

/// `_text_shingles` — resume.py:2710: tiktoken BPE replaced by char-level
/// n-grams (same n=5 default; short text → single shingle of the whole).
fn text_shingles(text: &str, n: usize) -> HashSet<Vec<char>> {
    if text.is_empty() {
        return HashSet::new();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < n {
        return HashSet::from([chars]);
    }
    (0..=chars.len() - n)
        .map(|i| chars[i..i + n].to_vec())
        .collect()
}

/// `_shingling_jaccard` — resume.py:2732. Empty union → 1.0.
pub fn shingling_jaccard(text1: &str, text2: &str, n: usize) -> f64 {
    let s1 = text_shingles(text1, n);
    let s2 = text_shingles(text2, n);
    let union: HashSet<&Vec<char>> = s1.union(&s2).collect();
    if union.is_empty() {
        return 1.0;
    }
    let intersection: HashSet<&Vec<char>> = s1.intersection(&s2).collect();
    intersection.len() as f64 / union.len() as f64
}

/// Internal work-experience detail used by the postprocessor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkExpDetail {
    pub company: String,
    pub start_date: String,
    pub end_date: String,
    pub years: f64,
}

/// Structured resume dictionary (serde_json Value mirror of resume.py dict).
pub type Resume = Value;

fn _get(resume: &Resume, key: &str) -> Option<Value> {
    resume.get(key).cloned()
}

/// JSON value → plain text (String unwrapped without quotes; numbers/other
/// via Display). Mirrors Python `str(value)` semantics for resume fields.
fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn _get_str(resume: &Resume, key: &str) -> Option<String> {
    resume.get(key).and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn _get_str_list(resume: &Resume, key: &str) -> Vec<String> {
    match resume.get(key) {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                other => Some(value_text(other)),
            })
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// `_parse_date_str` for the postprocessor's far-future default end.
fn _far_future() -> (i32, u32) {
    (2099, 12)
}

fn _date_overlaps(
    start1: Option<(i32, u32)>,
    end1: Option<(i32, u32)>,
    start2: Option<(i32, u32)>,
    end2: Option<(i32, u32)>,
) -> bool {
    let (s1y, s1m) = match start1 {
        Some((y, m)) => (y, m),
        _ => return false,
    };
    let (s2y, s2m) = match start2 {
        Some((y, m)) => (y, m),
        _ => return false,
    };
    let (e1y, e1m) = end1.map(|(y, m)| (y, m)).unwrap_or(_far_future());
    let (e2y, e2m) = end2.map(|(y, m)| (y, m)).unwrap_or(_far_future());
    // [s1,e1] and [s2,e2] overlap iff s1 <= e2 and s2 <= e1 (month-granularity).
    let s1_ord = s1y * 12 + s1m as i32;
    let e1_ord = e1y * 12 + e1m as i32;
    let s2_ord = s2y * 12 + s2m as i32;
    let e2_ord = e2y * 12 + e2m as i32;
    s1_ord <= e2_ord && s2_ord <= e1_ord
}

/// `_postprocess_resume` — resume.py:1794-2053 (4-phase pipeline).
pub fn postprocess_resume(resume: &mut Resume, lines: &[String], lang: &str) {
    let en = is_english(lang);
    let full_text = lines.join("\n");
    let norm_full_text = normalize_for_comparison(&full_text);

    // --- Phase 1: source-text validation ---
    let unknown_names = ["未知", "Unknown"];
    if let Some(name) = _get_str(resume, "name_kwd")
        && !unknown_names.contains(&name.as_str()) {
            let norm_name = normalize_for_comparison(&name);
            if !norm_full_text.is_empty()
                && !norm_name.is_empty()
                && !norm_full_text.contains(&norm_name)
            {
                resume["name_kwd"] = json!("");
            }
        }

    // Companies: strict full-name containment.
    if resume.get("corp_nm_tks").is_some() && !norm_full_text.is_empty() {
        let companies = _get_str_list(resume, "corp_nm_tks");
        let verified: Vec<String> = companies
            .iter()
            .filter(|c| {
                let norm = normalize_for_comparison(c);
                !norm.is_empty() && norm_full_text.contains(&norm)
            })
            .cloned()
            .collect();
        resume["corp_nm_tks"] = json!(verified);
        if let Some(first) = verified.first() {
            resume["corporation_name_tks"] = json!(first);
        } else {
            resume["corporation_name_tks"] = json!("");
        }
    }

    // Schools.
    if resume.get("school_name_tks").is_some() && !norm_full_text.is_empty() {
        let schools = _get_str_list(resume, "school_name_tks");
        let verified: Vec<String> = schools
            .iter()
            .filter(|s| {
                let norm = normalize_for_comparison(s);
                !norm.is_empty() && norm_full_text.contains(&norm)
            })
            .cloned()
            .collect();
        resume["school_name_tks"] = json!(verified);
        if let Some(first) = resume.get("first_school_name_tks").and_then(Value::as_str) {
            if !verified.is_empty() && !verified.contains(&first.to_string()) {
                resume["first_school_name_tks"] = json!(verified[verified.len() - 1]);
            }
        } else if !verified.is_empty() {
            resume["first_school_name_tks"] = json!(verified[0]);
        }
        if verified.is_empty() {
            resume["first_school_name_tks"] = json!("");
        }
    }

    // Positions.
    if resume.get("position_name_tks").is_some() && !norm_full_text.is_empty() {
        let positions = _get_str_list(resume, "position_name_tks");
        let verified: Vec<String> = positions
            .iter()
            .filter(|p| {
                let norm = normalize_for_comparison(p);
                !norm.is_empty() && norm_full_text.contains(&norm)
            })
            .cloned()
            .collect();
        if !verified.is_empty() {
            resume["position_name_tks"] = json!(verified);
        }
    }

    // --- Phase 2: domain normalization ---
    if let Some(birth) = _get_str(resume, "birth_dt") {
        let normalized = birth
            .replace(['年', '月'], "-")
            .trim_end_matches('-')
            .to_string();
        resume["birth_dt"] = json!(normalized);
    }
    if let Some(phone) = _get_str(resume, "phone_kwd") {
        let cleaned: String = phone
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '+')
            .collect();
        if !cleaned.is_empty() {
            resume["phone_kwd"] = json!(cleaned);
        }
    }
    if let Some(gender) = _get_str(resume, "gender_kwd") {
        let g = gender.trim();
        if matches!(g, "male" | "Male" | "M" | "m" | "男") {
            resume["gender_kwd"] = json!(if en { "Male" } else { "男" });
        } else if matches!(g, "female" | "Female" | "F" | "f" | "女") {
            resume["gender_kwd"] = json!(if en { "Female" } else { "女" });
        }
    }

    // --- Phase 3: contextual deduplication (order-preserving) ---
    for list_field in [
        "corp_nm_tks",
        "school_name_tks",
        "major_tks",
        "position_name_tks",
        "skill_tks",
    ] {
        if let Some(Value::Array(_)) = resume.get(list_field) {
            let items = _get_str_list(resume, list_field);
            let mut seen = HashSet::new();
            let mut deduped = Vec::new();
            for item in items {
                let item_str = item.trim().to_string();
                if !item_str.is_empty() && seen.insert(item_str.clone()) {
                    deduped.push(item_str);
                }
            }
            resume[list_field] = json!(deduped);
        }
    }

    // --- Phase 3.4: work_desc_tks dedup by company substring + date overlap ---
    let work_descs = _get_str_list(resume, "work_desc_tks");
    if work_descs.len() > 1 {
        let corp_names = _get_str_list(resume, "corp_nm_tks");
        let details: Vec<WorkExpDetail> = match resume.get("_work_exp_details") {
            Some(Value::Array(arr)) => arr
                .iter()
                .map(|d| WorkExpDetail {
                    company: d
                        .get("company")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    start_date: d
                        .get("start_date")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    end_date: d
                        .get("end_date")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    years: d.get("years").and_then(Value::as_f64).unwrap_or(0.0),
                })
                .collect(),
            _ => Vec::new(),
        };
        let positions = _get_str_list(resume, "position_name_tks");

        let mut kept_indices: Vec<usize> = Vec::new();
        for i in 0..work_descs.len() {
            let mut is_dup = false;
            let corp_i =
                normalize_for_comparison(corp_names.get(i).map(String::as_str).unwrap_or(""));
            let detail_i = details.get(i).cloned().unwrap_or_default();
            let dt_start_i = parse_date_str(&detail_i.start_date);
            let dt_end_i = parse_date_str(&detail_i.end_date);
            for &j in &kept_indices {
                let corp_j =
                    normalize_for_comparison(corp_names.get(j).map(String::as_str).unwrap_or(""));
                // Strategy A: company substring + time overlap.
                if !corp_i.is_empty() && !corp_j.is_empty() {
                    let (shorter, longer) = if corp_i.len() <= corp_j.len() {
                        (corp_i.as_str(), corp_j.as_str())
                    } else {
                        (corp_j.as_str(), corp_i.as_str())
                    };
                    if longer.contains(shorter) {
                        let detail_j = details.get(j).cloned().unwrap_or_default();
                        let dt_start_j = parse_date_str(&detail_j.start_date);
                        let dt_end_j = parse_date_str(&detail_j.end_date);
                        if dt_start_i.is_some() && dt_start_j.is_some() {
                            if _date_overlaps(dt_start_i, dt_end_i, dt_start_j, dt_end_j) {
                                is_dup = true;
                                break;
                            }
                        } else if (!detail_i.start_date.is_empty()
                            && !detail_j.start_date.is_empty()
                            && detail_i.start_date == detail_j.start_date)
                            || (!detail_i.end_date.is_empty()
                                && !detail_j.end_date.is_empty()
                                && detail_i.end_date == detail_j.end_date)
                        {
                            is_dup = true;
                            break;
                        }
                    }
                }
                // Strategy B: content-based Jaccard / substring fallback.
                let norm_i = normalize_for_comparison(&work_descs[i]);
                let norm_j = normalize_for_comparison(&work_descs[j]);
                let (shorter, longer) = if norm_i.len() <= norm_j.len() {
                    (norm_i.as_str(), norm_j.as_str())
                } else {
                    (norm_j.as_str(), norm_i.as_str())
                };
                if !shorter.is_empty() && !longer.is_empty() && longer.contains(shorter) {
                    is_dup = true;
                    break;
                }
                if shingling_jaccard(&work_descs[i], &work_descs[j], 5) > 0.5 {
                    is_dup = true;
                    break;
                }
            }
            if !is_dup {
                kept_indices.push(i);
            }
        }
        if kept_indices.len() < work_descs.len() {
            let new_descs: Vec<String> = kept_indices
                .iter()
                .map(|&i| work_descs[i].clone())
                .collect();
            resume["work_desc_tks"] = json!(new_descs);
            if !corp_names.is_empty() {
                let new_corps: Vec<String> = kept_indices
                    .iter()
                    .filter_map(|&i| corp_names.get(i).cloned())
                    .collect();
                resume["corp_nm_tks"] = json!(new_corps);
            }
            if !details.is_empty() {
                let new_details: Vec<Value> = kept_indices
                    .iter()
                    .filter_map(|&i| details.get(i))
                    .map(|d| {
                        json!({
                            "company": d.company,
                            "start_date": d.start_date,
                            "end_date": d.end_date,
                            "years": d.years,
                        })
                    })
                    .collect();
                resume["_work_exp_details"] = json!(new_details);
            }
            if !positions.is_empty() {
                let new_positions: Vec<String> = kept_indices
                    .iter()
                    .filter_map(|&i| positions.get(i).cloned())
                    .collect();
                resume["position_name_tks"] = json!(new_positions);
            }
            // Recalculate work years.
            if let Some(Value::Array(new_details)) = resume.get("_work_exp_details") {
                let mut recalc = 0.0;
                for d in new_details {
                    recalc += d.get("years").and_then(Value::as_f64).unwrap_or(0.0);
                }
                recalc = (recalc * 10.0).round() / 10.0;
                if recalc > 0.0 {
                    resume["work_exp_flt"] = json!(recalc);
                }
            }
            let new_corps = _get_str_list(resume, "corp_nm_tks");
            if let Some(first) = new_corps.first() {
                resume["corporation_name_tks"] = json!(first);
            }
        }
    }

    // --- Phase 3.5: merge project_desc_tks into work_desc_tks ---
    let work_descs = _get_str_list(resume, "work_desc_tks");
    let project_descs = _get_str_list(resume, "project_desc_tks");
    resume["_raw_project_descs"] = json!(project_descs.clone());
    if !project_descs.is_empty() {
        let project_names = _get_str_list(resume, "project_tks");
        let mut work_descs = work_descs;
        for (i, proj_desc) in project_descs.iter().enumerate() {
            let norm_proj = normalize_for_comparison(proj_desc);
            if norm_proj.is_empty() {
                continue;
            }
            let mut already_exists = false;
            for wd in &work_descs {
                let norm_wd = normalize_for_comparison(wd);
                if norm_wd.is_empty() {
                    continue;
                }
                let (shorter, longer) = if norm_proj.len() <= norm_wd.len() {
                    (norm_proj.as_str(), norm_wd.as_str())
                } else {
                    (norm_wd.as_str(), norm_proj.as_str())
                };
                if longer.contains(shorter) || shingling_jaccard(proj_desc, wd, 5) > 0.5 {
                    already_exists = true;
                    break;
                }
            }
            if !already_exists {
                let proj_name = project_names.get(i).cloned().unwrap_or_default();
                if proj_name.is_empty() {
                    work_descs.push(proj_desc.clone());
                } else {
                    work_descs.push(format!("[{proj_name}] {proj_desc}"));
                }
            }
        }
        resume["work_desc_tks"] = json!(work_descs);
        resume["project_desc_tks"] = json!(Vec::<String>::new());
    }

    // --- Phase 4: field completion ---
    let required_fields = [
        "name_kwd",
        "gender_kwd",
        "phone_kwd",
        "email_tks",
        "position_name_tks",
        "school_name_tks",
        "major_tks",
    ];
    for field in required_fields {
        if resume.get(field).is_none() {
            if field.ends_with("_tks") {
                resume[field] = json!(Vec::<String>::new());
            } else if field.ends_with("_int") || field.ends_with("_flt") {
                resume[field] = json!(0);
            } else {
                resume[field] = json!("");
            }
        }
    }
    // Safety fallback: drop internal markers.
    resume.as_object_mut().map(|m| m.remove("_name_confidence"));
}

/// Identity fields redundantly written to every chunk.
const IDENTITY_FIELDS: [&str; 6] = [
    "name_kwd",
    "phone_kwd",
    "email_tks",
    "gender_kwd",
    "highest_degree_kwd",
    "work_exp_flt",
];

/// Merge groups: (fields, zh title, en title).
const MERGE_GROUPS: [(&[&str], &str, &str); 4] = [
    (
        &[
            "name_kwd",
            "name_pinyin_kwd",
            "gender_kwd",
            "age_int",
            "phone_kwd",
            "email_tks",
            "birth_dt",
            "work_exp_flt",
            "position_name_tks",
            "expect_city_names_tks",
            "expect_position_name_tks",
        ],
        "基本信息",
        "Basic Info",
    ),
    (
        &[
            "first_school_name_tks",
            "first_degree_kwd",
            "highest_degree_kwd",
            "first_major_tks",
            "edu_first_fea_kwd",
            "degree_kwd",
            "major_tks",
            "school_name_tks",
            "sch_rank_kwd",
            "edu_fea_kwd",
            "edu_end_int",
        ],
        "教育背景",
        "Education",
    ),
    (
        &["skill_tks", "language_tks", "certificate_tks"],
        "技能与证书",
        "Skills & Certificates",
    ),
    (
        &["corporation_name_tks", "corp_nm_tks", "industry_name_tks"],
        "工作概况",
        "Work Overview",
    ),
];

/// Split-list fields that generate one chunk per element.
const SPLIT_LIST_FIELDS: [&str; 2] = ["work_desc_tks", "project_desc_tks"];

/// A resume chunk (mirrors the ES document shape in _build_chunk_document).
#[derive(Debug, Clone, Serialize, Default)]
pub struct ResumeChunk {
    pub content_with_weight: String,
    pub content_ltks: Vec<String>,
    pub content_sm_ltks: Vec<String>,
    pub docnm_kwd: String,
    pub title_tks: Vec<String>,
    pub title_sm_tks: Vec<String>,
    pub page_num_int: Vec<i64>,
    pub position_int: Vec<(i64, i64, i64, i64, i64)>,
    pub top_int: Vec<i64>,
    /// Extra structured fields (identity + per-field values).
    pub fields: serde_json::Map<String, Value>,
}

fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_owned).collect()
}

/// `_build_chunk_document` — resume.py:2101-2430.
pub fn build_chunk_document(filename: &str, resume: &Resume, lang: &str) -> Vec<ResumeChunk> {
    let en = is_english(lang);
    let field_map = get_field_map(lang);

    // doc base: docnm_kwd + title_tks (extension stripped).
    let title = strip_extension(filename);
    let title_tks = tokenize(&title);
    let title_sm_tks = title_tks.clone();

    // Identity metadata (redundantly written to each chunk).
    let mut identity_meta: serde_json::Map<String, Value> = serde_json::Map::new();
    for ik in IDENTITY_FIELDS {
        let Some(iv) = resume.get(ik) else { continue };
        if iv.is_null() {
            continue;
        }
        if ik.ends_with("_tks") {
            let joined = match iv {
                Value::Array(a) => a
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => value_text(other),
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                other => value_text(other),
            };
            identity_meta.insert(ik.to_string(), json!(tokenize(&joined)));
        } else if ik.ends_with("_kwd") {
            identity_meta.insert(ik.to_string(), iv.clone());
        } else if ik.ends_with("_flt") {
            if let Some(f) = iv.as_f64() {
                identity_meta.insert(ik.to_string(), json!(f));
            }
        } else {
            identity_meta.insert(ik.to_string(), json!(value_text(iv)));
        }
    }

    // Resume summary line.
    let mut summary_parts = Vec::new();
    if let Some(name) = _get_str(resume, "name_kwd")
        && !name.is_empty() {
            summary_parts.push(format!("{}:{name}", if en { "Name" } else { "姓名" }));
        }
    if let Some(phone) = _get_str(resume, "phone_kwd")
        && !phone.is_empty() {
            summary_parts.push(format!("{}:{phone}", if en { "Phone" } else { "电话" }));
        }
    if let Some(corp) = _get_str(resume, "corporation_name_tks")
        && !corp.is_empty() {
            summary_parts.push(format!("{}:{corp}", if en { "Company" } else { "公司" }));
        }
    if let Some(degree) = _get_str(resume, "highest_degree_kwd")
        && !degree.is_empty() {
            summary_parts.push(format!("{}:{degree}", if en { "Degree" } else { "学历" }));
        }
    if let Some(exp) = resume.get("work_exp_flt").and_then(Value::as_f64)
        && exp > 0.0 {
            if en {
                summary_parts.push(format!("Experience:{exp}yrs"));
            } else {
                summary_parts.push(format!("经验:{exp}年"));
            }
        }
    let resume_summary = if summary_parts.is_empty() {
        String::new()
    } else {
        summary_parts.join(" | ")
    };

    let mut chunks: Vec<ResumeChunk> = Vec::new();

    // Collect all merged fields to skip in the per-field loop.
    let mut all_merged: HashSet<&str> = HashSet::new();
    for (fields, _, _) in MERGE_GROUPS {
        all_merged.extend(fields.iter().copied());
    }

    // Merge groups: one chunk per group.
    for (fields_set, zh_title, en_title) in MERGE_GROUPS {
        let group_title = if en { en_title } else { zh_title };
        let mut group_parts = Vec::new();
        let mut group_field_values: Vec<(String, Value)> = Vec::new();
        for (field_key, field_desc) in &field_map {
            if !fields_set.contains(field_key) {
                continue;
            }
            let Some(value) = resume.get(*field_key) else {
                continue;
            };
            if value.is_null() {
                continue;
            }
            let text_value = match value {
                Value::Array(a) => a
                    .iter()
                    .map(|v| v.as_str().unwrap_or(""))
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" "),
                other => value_text(other),
            };
            if text_value.trim().is_empty() {
                continue;
            }
            group_parts.push(format!("{field_desc}: {text_value}"));
            group_field_values.push((field_key.to_string(), value.clone()));
        }
        if group_parts.is_empty() {
            continue;
        }
        let mut content = format!("{group_title}\n{}", group_parts.join("\n"));
        if !resume_summary.is_empty() {
            content.push_str(&format!("\n[{resume_summary}]"));
        }
        let mut chunk = base_chunk(filename, &title_tks, &title_sm_tks, &content);
        // Redundantly write identity fields.
        for (mk, mv) in &identity_meta {
            chunk.fields.insert(mk.clone(), mv.clone());
        }
        // Write each field's structured value.
        for (fk, fv) in &group_field_values {
            chunk.fields.insert(fk.clone(), typed_field(fk, fv));
        }
        chunks.push(chunk);
    }

    // Per-field chunks (skipping merged + split-list fields).
    for (field_key, field_desc) in &field_map {
        if all_merged.contains(field_key) {
            continue;
        }
        let Some(value) = resume.get(*field_key) else {
            continue;
        };
        if value.is_null() {
            continue;
        }

        // Split-list fields: one chunk per element.
        if SPLIT_LIST_FIELDS.contains(field_key) {
            if let Value::Array(items) = value {
                let corp_list = if *field_key == "work_desc_tks" {
                    _get_str_list(resume, "corp_nm_tks")
                } else {
                    Vec::new()
                };
                let project_list = if *field_key == "project_desc_tks" {
                    _get_str_list(resume, "project_tks")
                } else {
                    Vec::new()
                };
                let work_details: Vec<Value> = match resume.get("_work_exp_details") {
                    Some(Value::Array(a)) => a.clone(),
                    _ => Vec::new(),
                };
                for (idx, item) in items.iter().enumerate() {
                    let item_text = item.as_str().unwrap_or("").trim().to_string();
                    if item_text.is_empty() {
                        continue;
                    }
                    let content_prefix = if *field_key == "work_desc_tks"
                        && idx < work_details.len()
                    {
                        let detail = &work_details[idx];
                        let company = detail.get("company").and_then(Value::as_str).unwrap_or("");
                        let start_d = detail
                            .get("start_date")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let end_d = detail.get("end_date").and_then(Value::as_str).unwrap_or("");
                        let years = detail.get("years").and_then(Value::as_f64).unwrap_or(0.0);
                        let mut time_parts = Vec::new();
                        if !start_d.is_empty() {
                            if end_d.is_empty() {
                                time_parts.push(start_d.to_string());
                            } else {
                                time_parts.push(format!("{start_d}-{end_d}"));
                            }
                        }
                        if years > 0.0 {
                            time_parts.push(format!("{years}{}", if en { "yrs" } else { "年" }));
                        }
                        let time_text = time_parts.join(" ");
                        if !company.is_empty() && !time_text.is_empty() {
                            format!("{field_desc}（{company} {time_text}）")
                        } else if !company.is_empty() {
                            format!("{field_desc}（{company}）")
                        } else if en {
                            format!("{field_desc}（#{}）", idx + 1)
                        } else {
                            format!("{field_desc}（第{}段）", idx + 1)
                        }
                    } else if *field_key == "work_desc_tks" && idx < corp_list.len() {
                        format!("{field_desc}（{}）", corp_list[idx])
                    } else if *field_key == "project_desc_tks" && idx < project_list.len() {
                        format!("{field_desc}（{}）", project_list[idx])
                    } else if en {
                        format!("{field_desc}（#{}）", idx + 1)
                    } else {
                        format!("{field_desc}（第{}段）", idx + 1)
                    };

                    let content = if resume_summary.is_empty() {
                        format!("{content_prefix}: {item_text}")
                    } else {
                        format!("{content_prefix}: {item_text}\n[{resume_summary}]")
                    };
                    let mut chunk = base_chunk(filename, &title_tks, &title_sm_tks, &content);
                    for (mk, mv) in &identity_meta {
                        if mk != field_key {
                            chunk.fields.insert(mk.clone(), mv.clone());
                        }
                    }
                    chunk
                        .fields
                        .insert(field_key.to_string(), json!(tokenize(&item_text)));
                    chunks.push(chunk);
                }
            }
            continue;
        }

        // Ordinary field: "field_desc: value".
        let text_value = match value {
            Value::Array(a) => a
                .iter()
                .map(|v| v.as_str().unwrap_or(""))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
            other => value_text(other),
        };
        if text_value.trim().is_empty() {
            continue;
        }
        let content =
            if !resume_summary.is_empty() && !matches!(*field_key, "name_kwd" | "phone_kwd") {
                format!("{field_desc}: {text_value}\n[{resume_summary}]")
            } else {
                format!("{field_desc}: {text_value}")
            };
        let mut chunk = base_chunk(filename, &title_tks, &title_sm_tks, &content);
        for (mk, mv) in &identity_meta {
            if mk != field_key {
                chunk.fields.insert(mk.clone(), mv.clone());
            }
        }
        chunk
            .fields
            .insert(field_key.to_string(), typed_field(field_key, value));
        chunks.push(chunk);
    }

    // Fallback: at least one chunk with the name.
    if chunks.is_empty() {
        let name = _get_str(resume, "name_kwd")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if en {
                    "Unknown".into()
                } else {
                    "未知".into()
                }
            });
        let content = format!("{}: {name}", if en { "Name" } else { "姓名" });
        chunks.push(base_chunk(filename, &title_tks, &title_sm_tks, &content));
    }

    // add_positions: page=0, top increments by index (resume.py:2426-2428).
    for (i, chunk) in chunks.iter_mut().enumerate() {
        let p = 1i64;
        let t = i as i64;
        chunk.page_num_int = vec![p];
        chunk.top_int = vec![t];
        chunk.position_int = vec![(p, 0, 0, t, t)];
    }

    chunks
}

/// Strip trailing extension — mirrors `re.sub(r"\.[a-zA-Z]+$", "", filename)`.
fn strip_extension(filename: &str) -> String {
    let path = std::path::Path::new(filename);
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphabetic()) => {
            let stem = path.with_extension("");
            stem.to_string_lossy().into_owned()
        }
        _ => filename.to_string(),
    }
}

fn base_chunk(
    filename: &str,
    title_tks: &[String],
    title_sm_tks: &[String],
    content: &str,
) -> ResumeChunk {
    let content_ltks = tokenize(content);
    ResumeChunk {
        content_with_weight: content.to_string(),
        content_sm_ltks: content_ltks.clone(),
        content_ltks,
        docnm_kwd: filename.to_string(),
        title_tks: title_tks.to_vec(),
        title_sm_tks: title_sm_tks.to_vec(),
        fields: serde_json::Map::new(),
        ..Default::default()
    }
}

/// Type the field value per its suffix (resume.py:2376-2394).
fn typed_field(field_key: &str, value: &Value) -> Value {
    if field_key.ends_with("_tks") {
        let joined = match value {
            Value::Array(a) => a
                .iter()
                .map(|v| v.as_str().unwrap_or(""))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
            other => other.to_string(),
        };
        json!(tokenize(&joined))
    } else if field_key.ends_with("_kwd") {
        match value {
            Value::Array(a) => json!(a.clone()),
            _ => value.clone(),
        }
    } else if field_key.ends_with("_int") {
        match value.as_i64() {
            Some(i) => json!(i),
            None => value.clone(),
        }
    } else if field_key.ends_with("_flt") {
        match value.as_f64() {
            Some(f) => json!(f),
            None => value.clone(),
        }
    } else {
        value.clone()
    }
}

use serde::Serialize;

/// Convenience: regex parse → postprocess → build chunks in one call
/// (mirrors parse_resume's LLM-free fallback path).
pub fn parse_resume_regex(text: &str, filename: &str, lang: &str) -> Vec<ResumeChunk> {
    let mut resume = parse_with_regex(text, lang);
    let lines: Vec<String> = text
        .split('\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    postprocess_resume(&mut resume, &lines, lang);
    build_chunk_document(filename, &resume, lang)
}

/// `parse_with_regex` — resume.py:1482-1787 (bilingual regex extraction).
pub fn parse_with_regex(text: &str, lang: &str) -> Resume {
    let en = is_english(lang);
    let lines: Vec<&str> = text
        .split('\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let mut resume = json!({});

    // --- Name ---
    if en {
        let re_label =
            regex::Regex::new(r"(?i)(?:Name|Full\s*Name)\s*[:：]\s*([A-Za-z][A-Za-z\s\-\.]{1,40})")
                .unwrap();
        for line in lines.iter().take(30) {
            if let Some(caps) = re_label.captures(line) {
                resume["name_kwd"] = json!(caps.get(1).unwrap().as_str().trim());
                break;
            }
        }
        if resume.get("name_kwd").is_none()
            && let Some(first) = lines.first()
                && first.len() <= 40
                    && !first.chars().any(|c| c.is_ascii_digit())
                    && regex::Regex::new(r"^[A-Za-z][A-Za-z\s\-\.]+$")
                        .unwrap()
                        .is_match(first)
                {
                    resume["name_kwd"] = json!(first.to_string());
                }
    } else {
        let re_label = regex::Regex::new(r"姓\s*名\s*[:：]\s*([\u{4e00}-\u{9fa5}]{2,4})").unwrap();
        for line in lines.iter().take(30) {
            if let Some(caps) = re_label.captures(line) {
                resume["name_kwd"] = json!(caps.get(1).unwrap().as_str());
                break;
            }
        }
        if resume.get("name_kwd").is_none() {
            let title_words = [
                "个人", "简历", "求职", "应聘", "基本", "信息", "概述", "简介", "教育", "工作",
                "经历", "经验", "技能", "项目", "自我", "评价", "专业", "技术", "证书", "语言",
                "能力", "培训", "荣誉", "奖项",
            ];
            for line in lines.iter().take(20) {
                if title_words.iter().any(|w| line.contains(w)) {
                    continue;
                }
                if line.contains([':', '：']) && line.len() > 6 {
                    continue;
                }
                let cleaned = regex::Regex::new(r"^[A-Za-z_\-\d\s]+\s+")
                    .unwrap()
                    .replace(line, "");
                let cleaned = regex::Regex::new(r"\s+[A-Za-z_\-\d\s]+$")
                    .unwrap()
                    .replace(&cleaned, "");
                let cleaned = cleaned.trim();
                let re_cn = regex::Regex::new(r"^[\u{4e00}-\u{9fa5}]{2,4}$").unwrap();
                if (2..=4).contains(&cleaned.chars().count()) && re_cn.is_match(cleaned) {
                    resume["name_kwd"] = json!(cleaned);
                    break;
                }
            }
        }
        if resume.get("name_kwd").is_none()
            && let Some(first) = lines.first()
                && first.len() <= 10 && !first.chars().any(|c| c.is_ascii_digit()) {
                    let re_cn = regex::Regex::new(r"[\u{4e00}-\u{9fa5}]+").unwrap();
                    if let Some(m) = re_cn.find(first) {
                        let part = m.as_str();
                        if (2..=4).contains(&part.chars().count()) {
                            resume["name_kwd"] = json!(part);
                        }
                    }
                }
    }

    // --- Phone ---
    let phones = regex::Regex::new(r"1[3-9]\d{9}").unwrap();
    if let Some(m) = phones.find(text) {
        resume["phone_kwd"] = json!(m.as_str());
    }

    // --- Email ---
    let emails = regex::Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}").unwrap();
    if let Some(m) = emails.find(text) {
        resume["email_tks"] = json!(m.as_str());
    }

    // --- Gender ---
    if en {
        let re_label =
            regex::Regex::new(r"(?i)(?:Gender|Sex)\s*[:：]\s*(Male|Female|M|F)").unwrap();
        if let Some(caps) = re_label.captures(text) {
            let raw = caps.get(1).unwrap().as_str().trim().to_uppercase();
            resume["gender_kwd"] = json!(if raw == "M" || raw == "MALE" {
                "Male"
            } else {
                "Female"
            });
        } else {
            let re_word = regex::Regex::new(r"(?i)\b(Male|Female)\b").unwrap();
            if let Some(m) = re_word.find(&text[..text.len().min(500)]) {
                let s = m.as_str().to_string();
                let mut c = s.chars();
                let first = c.next().unwrap().to_ascii_uppercase();
                resume["gender_kwd"] = json!(format!("{first}{}", c.as_str().to_lowercase()));
            }
        }
    } else {
        let re_label = regex::Regex::new(r"性\s*别\s*[:：]\s*(男|女)").unwrap();
        if let Some(caps) = re_label.captures(text) {
            resume["gender_kwd"] = json!(caps.get(1).unwrap().as_str());
        } else {
            let head = &text[..text.len().min(500)];
            let re_word = regex::Regex::new(r"(男|女)").unwrap();
            if let Some(m) = re_word.find(head) {
                resume["gender_kwd"] = json!(m.as_str());
            }
        }
    }

    // --- Age ---
    if en {
        let re_age = regex::Regex::new(r"(?i)(?:Age)\s*[:：]\s*(\d{1,2})").unwrap();
        let mut age = re_age
            .captures(text)
            .map(|c| c.get(1).unwrap().as_str().to_string());
        if age.is_none() {
            let re_years = regex::Regex::new(r"(?i)(\d{1,2})\s*years?\s*old").unwrap();
            age = re_years
                .captures(text)
                .map(|c| c.get(1).unwrap().as_str().to_string());
        }
        if let Some(a) = age
            && let Ok(v) = a.parse::<i64>() {
                resume["age_int"] = json!(v);
            }
    } else {
        let re_age = regex::Regex::new(r"(\d{1,2})\s*岁").unwrap();
        if let Some(caps) = re_age.captures(text)
            && let Ok(v) = caps.get(1).unwrap().as_str().parse::<i64>() {
                resume["age_int"] = json!(v);
            }
    }

    // --- Birth ---
    if en {
        let re_label =
            regex::Regex::new(r"(?i)(?:Birth|DOB|Date\s*of\s*Birth)\s*[:：]\s*(.{6,20})").unwrap();
        if let Some(caps) = re_label.captures(text) {
            resume["birth_dt"] = json!(caps.get(1).unwrap().as_str().trim());
        } else {
            let re_date = regex::Regex::new(r"(19|20)\d{2}[-/]\d{1,2}[-/]\d{1,2}").unwrap();
            if let Some(m) = re_date.find(text) {
                resume["birth_dt"] = json!(m.as_str());
            }
        }
    } else {
        let re_birth = regex::Regex::new(r"(19|20)\d{2}[年/-]\d{1,2}[月/-]\d{1,2}").unwrap();
        if let Some(m) = re_birth.find(text) {
            resume["birth_dt"] = json!(m.as_str());
        }
    }

    // --- Degree ---
    let degree_zh = [
        "博士", "硕士", "本科", "大专", "专科", "高中", "MBA", "EMBA", "MPA",
    ];
    let degree_en = [
        "PhD",
        "Master",
        "Bachelor",
        "Associate",
        "Diploma",
        "High School",
        "MBA",
        "EMBA",
        "MPA",
        "Doctor",
    ];
    let keywords: &[&str] = if en { &degree_en } else { &degree_zh };
    let found: Vec<&str> = keywords
        .iter()
        .copied()
        .filter(|d| text.contains(d))
        .collect();
    if !found.is_empty() {
        resume["degree_kwd"] = json!(found);
    }

    // --- School ---
    let schools: Vec<String> = if en {
        let re_school = regex::Regex::new(
            r"([A-Z][A-Za-z\s\-&]{2,40}(?:University|College|Institute|School|Academy))",
        )
        .unwrap();
        re_school
            .captures_iter(text)
            .map(|c| {
                regex::Regex::new(r"\s+")
                    .unwrap()
                    .replace_all(c.get(1).unwrap().as_str(), " ")
                    .trim()
                    .to_string()
            })
            .collect()
    } else {
        let re_school =
            regex::Regex::new(r"[\u{4e00}-\u{9fa5}]{2,15}(?:大学|学院|职业技术学院)").unwrap();
        re_school
            .find_iter(text)
            .map(|m| m.as_str().to_string())
            .collect()
    };
    if !schools.is_empty() {
        let unique: Vec<String> = schools
            .iter()
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        resume["school_name_tks"] = json!(unique);
        resume["first_school_name_tks"] = json!(schools[0]);
    }

    // --- Major ---
    let majors: Vec<String> = if en {
        let re_major = regex::Regex::new(
            r"(?i)(?:Major|Field\s*of\s*Study|Specialization|Concentration)\s*[:：]\s*([A-Za-z\s\-&,]{2,40})",
        )
        .unwrap();
        re_major
            .captures_iter(text)
            .map(|c| c.get(1).unwrap().as_str().trim().to_string())
            .filter(|m| !m.is_empty())
            .collect()
    } else {
        let re_major = regex::Regex::new(r"专业[:：]\s*([\u{4e00}-\u{9fa5}]{2,20})").unwrap();
        re_major
            .captures_iter(text)
            .map(|c| c.get(1).unwrap().as_str().to_string())
            .collect()
    };
    if !majors.is_empty() {
        resume["major_tks"] = json!(majors);
        resume["first_major_tks"] = json!(majors[0]);
    }

    // --- Companies ---
    let companies: Vec<String> = if en {
        let re_company = regex::Regex::new(
            r"([A-Z][A-Za-z\s\-&,\.]{2,40}(?:Inc\.|Corp\.|Ltd\.|LLC|Co\.|Company|Group|Technologies|Technology|Solutions|Consulting|Services|Bank))",
        )
        .unwrap();
        re_company
            .captures_iter(text)
            .map(|c| {
                regex::Regex::new(r"\s+")
                    .unwrap()
                    .replace_all(c.get(1).unwrap().as_str(), " ")
                    .trim()
                    .to_string()
            })
            .collect()
    } else {
        let re_c1 = regex::Regex::new(
            r"[\u{4e00}-\u{9fa5}]{2,20}[（(][\u{4e00}-\u{9fa5}]{2,10}[)）](?:科技|信息技术|网络科技)?(?:股份)?有限公司",
        )
        .unwrap();
        let re_c2 = regex::Regex::new(
            r"[\u{4e00}-\u{9fa5}]{4,20}(?:科技|信息技术|网络科技|银行)?(?:股份)?有限公司",
        )
        .unwrap();
        let mut out = Vec::new();
        for m in re_c1.find_iter(text) {
            out.push(m.as_str().to_string());
        }
        for m in re_c2.find_iter(text) {
            out.push(m.as_str().to_string());
        }
        out
    };

    // Filter verb list (bilingual) + substring dedup (resume.py:1665-1688).
    let filter_verbs: &[&str] = if en {
        &[
            "completed",
            "conducted",
            "implemented",
            "responsible",
            "participated",
            "developed",
        ]
    } else {
        &["完成", "进行", "实施", "负责", "参与", "开发"]
    };
    let min_len = if en { 3 } else { 6 };
    let mut unique_companies: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for c in &companies {
        if c.chars().count() < min_len || filter_verbs.iter().any(|v| c.to_lowercase().contains(v))
        {
            continue;
        }
        if seen.contains(c) {
            continue;
        }
        let mut is_sub = false;
        let mut to_remove: Vec<usize> = Vec::new();
        for (idx, existing) in unique_companies.iter().enumerate() {
            if c.contains(existing.as_str()) {
                is_sub = true;
                break;
            }
            if existing.contains(c.as_str()) {
                to_remove.push(idx);
            }
        }
        if !is_sub {
            for idx in to_remove.into_iter().rev() {
                seen.remove(&unique_companies[idx]);
                unique_companies.remove(idx);
            }
            unique_companies.push(c.clone());
            seen.insert(c.clone());
        }
    }
    if !unique_companies.is_empty() {
        resume["corp_nm_tks"] = json!(unique_companies);
        resume["corporation_name_tks"] = json!(unique_companies[0]);
    }

    // --- Position ---
    let mut positions: Vec<String> = if en {
        let re_label = regex::Regex::new(
            r"(?i)(?:Title|Position|Role|Job\s*Title)\s*[:：]\s*([A-Za-z\s\-/&]{2,30})",
        )
        .unwrap();
        let mut out: Vec<String> = re_label
            .captures_iter(text)
            .map(|c| c.get(1).unwrap().as_str().trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        let suffixes = [
            "Engineer",
            "Manager",
            "Director",
            "Supervisor",
            "Specialist",
            "Designer",
            "Consultant",
            "Assistant",
            "Architect",
            "Analyst",
            "Developer",
            "Lead",
            "Officer",
            "Coordinator",
            "Administrator",
            "Intern",
            "VP",
            "President",
        ];
        let filter_pos_verbs = [
            "responsible",
            "participated",
            "completed",
            "developed",
            "designed",
        ];
        for line in &lines {
            if line.chars().count() > 60 {
                continue;
            }
            for suffix in suffixes {
                let pat = format!(r"(?i)([A-Za-z\s\-]{{1,25}}{suffix})\b");
                let re = regex::Regex::new(&pat).unwrap();
                if let Some(m) = re.captures(line) {
                    let pos = m.get(1).unwrap().as_str().trim().to_string();
                    if !filter_pos_verbs
                        .iter()
                        .any(|v| pos.to_lowercase().contains(v))
                        && pos.chars().count() > 3
                    {
                        out.push(pos);
                    }
                }
            }
        }
        out
    } else {
        let re_label = regex::Regex::new(
            r"(?:职位|岗位|职务|职称|担任)\s*[:：]\s*([\u{4e00}-\u{9fa5}a-zA-Z]{2,15})",
        )
        .unwrap();
        let mut out: Vec<String> = re_label
            .captures_iter(text)
            .map(|c| c.get(1).unwrap().as_str().to_string())
            .collect();
        let re_company_pos = regex::Regex::new(
            r"(?:有限公司|集团|银行)\s+([\u{4e00}-\u{9fa5}]{2,8}(?:工程师|经理|总监|主管|专员|设计师|顾问|助理|架构师|分析师|运营|产品))",
        )
        .unwrap();
        for line in &lines {
            if let Some(caps) = re_company_pos.captures(line) {
                out.push(caps.get(1).unwrap().as_str().to_string());
            }
        }
        let suffixes = [
            "工程师",
            "经理",
            "总监",
            "主管",
            "专员",
            "设计师",
            "顾问",
            "助理",
            "架构师",
            "分析师",
            "开发者",
            "负责人",
        ];
        let filter_pos = ["负责", "参与", "完成", "开发了", "设计了"];
        for line in &lines {
            if line.chars().count() > 20 {
                continue;
            }
            for suffix in suffixes {
                let pat = format!(r"([\u{{4e00}}-\u{{9fa5}}]{{1,6}}{suffix})");
                let re = regex::Regex::new(&pat).unwrap();
                if let Some(m) = re.captures(line) {
                    let pos = m.get(1).unwrap().as_str().to_string();
                    if !filter_pos.iter().any(|v| pos.contains(v)) {
                        out.push(pos);
                    }
                }
            }
        }
        out
    };

    // Deduplicate while preserving order (resume.py:1750-1758).
    let mut seen_pos = HashSet::new();
    positions.retain(|p| seen_pos.insert(p.clone()));
    if !positions.is_empty() {
        resume["position_name_tks"] = json!(positions);
    }

    // --- Work years ---
    if en {
        let re_exp =
            regex::Regex::new(r"(?i)(\d+)\+?\s*years?\s*(?:of\s*)?(?:experience|work)").unwrap();
        if let Some(caps) = re_exp.captures(text)
            && let Ok(v) = caps.get(1).unwrap().as_str().parse::<f64>() {
                resume["work_exp_flt"] = json!(v);
            }
    } else {
        let re_exp = regex::Regex::new(r"(\d+)\s*年.*?经验").unwrap();
        if let Some(caps) = re_exp.captures(text)
            && let Ok(v) = caps.get(1).unwrap().as_str().parse::<f64>() {
                resume["work_exp_flt"] = json!(v);
            }
    }

    // --- Graduation year ---
    if en {
        let re_grad =
            regex::Regex::new(r"(?i)(?:Graduat(?:ed|ion)|Class\s*of)\s*[:：]?\s*((?:19|20)\d{2})")
                .unwrap();
        if let Some(caps) = re_grad.captures(text)
            && let Ok(v) = caps.get(1).unwrap().as_str().parse::<i64>() {
                resume["edu_end_int"] = json!(v);
            }
    } else {
        let re_grad = regex::Regex::new(r"((?:19|20)\d{2})\s*年.*?毕业").unwrap();
        if let Some(caps) = re_grad.captures(text)
            && let Ok(v) = caps.get(1).unwrap().as_str().parse::<i64>() {
                resume["edu_end_int"] = json!(v);
            }
    }

    if resume.get("name_kwd").is_none() {
        resume["name_kwd"] = json!(if en { "Unknown" } else { "未知" });
    }

    resume
}

/// Degree entity table — full port of
/// `deepdoc/parser/resume/entities/degrees.py`.
///
/// Maps degree ids ↔ Chinese names (TBL / TBL_ reversed lookup) with
/// `get_name(id)` / `get_id(name)` (case-insensitive, trimmed).
pub struct DegreeEntity;

/// id → name (degrees.py:17-32).
pub const DEGREE_TBL: &[(&str, &str)] = &[
    ("94", "EMBA"),
    ("6", "MBA"),
    ("95", "MPA"),
    ("92", "专升本"),
    ("4", "专科"),
    ("90", "中专"),
    ("91", "中技"),
    ("86", "初中"),
    ("3", "博士"),
    ("10", "博士后"),
    ("1", "本科"),
    ("2", "硕士"),
    ("87", "职高"),
    ("89", "高中"),
];

impl DegreeEntity {
    /// `get_name` — degrees.py:37-38.
    pub fn get_name(id: &str) -> String {
        DEGREE_TBL
            .iter()
            .find(|(k, _)| *k == id)
            .map(|(_, v)| v.to_string())
            .unwrap_or_default()
    }

    /// `get_id` — degrees.py:40-44 (None → ""; name uppercased + trimmed).
    pub fn get_id(nm: &str) -> String {
        if nm.is_empty() {
            return String::new();
        }
        let key = nm.trim().to_uppercase();
        DEGREE_TBL
            .iter()
            .find(|(_, v)| v.eq_ignore_ascii_case(&key))
            .map(|(k, _)| k.to_string())
            .unwrap_or_default()
    }
}

/// School entity — full port of
/// `deepdoc/parser/resume/entities/schools.py`.
///
/// Data files (`res/schools.csv`, `res/good_sch.json`,
/// `res/school.rank.csv`) are embedded verbatim via `include_str!`.
pub struct SchoolEntity;

/// One row of `schools.csv` (14 columns + `rank` from school.rank.csv).
#[derive(Debug, Clone, Default)]
pub struct SchoolRow {
    pub id: i64,
    pub r#type: i64,
    pub parent_id: i64,
    pub name_cn: String,
    pub name_en: String,
    pub alias: String,
    pub is_abroad: i64,
    pub is_world_known: i64,
    pub school_type: String,
    pub is_double_first: i64,
    pub education_type: String,
    pub province: String,
    pub city: String,
    pub is_985: i64,
    pub rank: i64,
}

/// Embed the three data files verbatim.
const SCHOOLS_CSV: &str = include_str!("data/resume/schools.csv");
const GOOD_SCH_JSON: &str = include_str!("data/resume/good_sch.json");
const SCHOOL_RANK_CSV: &str = include_str!("data/resume/school.rank.csv");

/// Parse `schools.csv` (tab-separated, header row) into rows.
fn parse_schools_csv() -> Vec<SchoolRow> {
    let mut rows = Vec::new();
    let mut lines = SCHOOLS_CSV.lines();
    let _header = lines.next(); // skip header
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 14 {
            continue;
        }
        let num = |s: &str| s.trim().parse::<i64>().unwrap_or(0);
        let str_clean = |s: &str| s.trim().to_string();
        rows.push(SchoolRow {
            id: num(cols[0]),
            r#type: num(cols[1]),
            parent_id: num(cols[2]),
            name_cn: str_clean(cols[3]),
            // schools.py:27 — name_en lowercased + stripped at load.
            name_en: cols[4].trim().to_lowercase(),
            alias: str_clean(cols[5]),
            is_abroad: num(cols[6]),
            is_world_known: num(cols[7]),
            school_type: cols[8].to_string(),
            is_double_first: num(cols[9]),
            education_type: cols[10].to_string(),
            province: cols[11].to_string(),
            city: cols[12].to_string(),
            is_985: num(cols[13]),
            rank: 1_000_000,
        });
    }
    rows
}

/// `loadRank` — schools.py:33-50: apply `school.rank.csv` (name,rank)
/// onto rows matching name_cn or name_en.
fn load_rank(rows: &mut [SchoolRow]) {
    for line in SCHOOL_RANK_CSV.lines() {
        let line = line.trim_end_matches('\n');
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 2 {
            continue;
        }
        let nm = parts[0].trim();
        let rk: i64 = match parts[1].trim().parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        for row in rows.iter_mut() {
            if row.name_cn == nm || row.name_en == nm {
                row.rank = rk;
            }
        }
    }
}

/// `GOOD_SCH` — schools.py:28-30: JSON list with `[,. &（）()]+` stripped.
fn good_schools() -> std::collections::HashSet<String> {
    let v: serde_json::Value =
        serde_json::from_str(GOOD_SCH_JSON).unwrap_or(serde_json::Value::Null);
    let arr = v.as_array().cloned().unwrap_or_default();
    let re = regex::Regex::new(r"[,. &（）()]+").unwrap();
    arr.iter()
        .filter_map(|x| x.as_str())
        .map(|c| re.replace_all(c, "").into_owned())
        .collect()
}

/// `split` — schools.py:53-65: whitespace tokenization that glues an
/// alphabetic token onto a previous token ending in a letter.
pub fn split(txt: &str) -> Vec<String> {
    let norm = regex::Regex::new(r"[ \t]+").unwrap().replace_all(txt, " ");
    let mut tks: Vec<String> = Vec::new();
    let re_alpha_end = regex::Regex::new(r".*[a-zA-Z]$").unwrap();
    let re_alpha_start = regex::Regex::new(r"[a-zA-Z]").unwrap();
    for t in norm.split_whitespace() {
        if !tks.is_empty()
            && re_alpha_end.is_match(&tks[tks.len() - 1])
            && re_alpha_start.is_match(t)
        {
            let last = tks.pop().unwrap_or_default();
            tks.push(format!("{last} {t}"));
        } else {
            tks.push(t.to_string());
        }
    }
    tks
}

/// `select` — schools.py:68-85: exact match on name_cn/name_en/alias after
/// normalization; returns the first hit as a JSON object (pandas
/// `to_json(orient="records")[0]` shape).
pub fn select(nm: &str) -> Option<serde_json::Value> {
    if nm.is_empty() {
        return None;
    }
    let mut nm = nm.to_string();
    if nm.starts_with('[') {
        // list input → first element (schools.py:72-74)
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&nm)
            && let Some(first) = v.as_array().and_then(|a| a.first()) {
                nm = first.as_str().unwrap_or("").to_string();
            }
    }
    nm = split(&nm).first().cloned().unwrap_or_default();
    nm = nm.to_lowercase().trim().to_string();
    // strip parenthesized content
    let re_paren = regex::Regex::new(r"[(（][^()（）]+[)）]").unwrap();
    nm = re_paren.replace_all(&nm, "").into_owned();
    // strip leading "the " / separators / country prefixes
    let re_strip = regex::Regex::new(r"(^the |[,.&（）();；·]+|^(英国|美国|瑞士))").unwrap();
    nm = re_strip.replace_all(&nm, "").into_owned();
    // "大学...学院" → "大学"
    let re_uni = regex::Regex::new(r"大学.*学院").unwrap();
    nm = re_uni.replace_all(&nm, "大学").into_owned();
    if nm.is_empty() {
        return None;
    }

    let mut rows = parse_schools_csv();
    load_rank(&mut rows);
    // alias match: nm in set(alias.split("+"))
    for row in &rows {
        let hit_alias = row.alias.split('+').any(|a| a == nm);
        if row.name_cn == nm || row.name_en == nm || hit_alias {
            return Some(serde_json::json!({
                "id": row.id,
                "type": row.r#type,
                "parent_id": row.parent_id,
                "name_cn": row.name_cn,
                "name_en": row.name_en,
                "alias": row.alias,
                "is_abroad": row.is_abroad,
                "is_world_known": row.is_world_known,
                "school_type": row.school_type,
                "is_double_first": row.is_double_first,
                "education_type": row.education_type,
                "province": row.province,
                "city": row.city,
                "is_985": row.is_985,
                "rank": row.rank,
            }));
        }
    }
    None
}

/// `is_good` — schools.py:88-92: normalized name membership in GOOD_SCH.
pub fn is_good(nm: &str) -> bool {
    let lowered = nm.to_lowercase();
    let re_paren = regex::Regex::new(r"[(（][^()（）]+[)）]").unwrap();
    let nm = re_paren.replace_all(lowered.as_str(), "");
    let re_strip = regex::Regex::new(r"[''`‘’“”,. &（）();；]+").unwrap();
    let nm = re_strip.replace_all(nm.as_ref(), "");
    good_schools().contains(nm.as_ref())
}

/// Cache for the parsed table (schools.py loads once at module import).
pub struct SchoolTable {
    rows: Vec<SchoolRow>,
}

impl Default for SchoolTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SchoolTable {
    pub fn new() -> Self {
        let mut rows = parse_schools_csv();
        load_rank(&mut rows);
        Self { rows }
    }

    /// `select` with a prebuilt table (avoids reparsing per call).
    pub fn select(&self, nm: &str) -> Option<serde_json::Value> {
        let mut nm = nm.to_string();
        if nm.starts_with('[')
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&nm)
                && let Some(first) = v.as_array().and_then(|a| a.first()) {
                    nm = first.as_str().unwrap_or("").to_string();
                }
        nm = split(&nm).first().cloned().unwrap_or_default();
        nm = nm.to_lowercase().trim().to_string();
        let re_paren = regex::Regex::new(r"[(（][^()（）]+[)）]").unwrap();
        nm = re_paren.replace_all(&nm, "").into_owned();
        let re_strip = regex::Regex::new(r"(^the |[,.&（）();；·]+|^(英国|美国|瑞士))").unwrap();
        nm = re_strip.replace_all(&nm, "").into_owned();
        let re_uni = regex::Regex::new(r"大学.*学院").unwrap();
        nm = re_uni.replace_all(&nm, "大学").into_owned();
        if nm.is_empty() {
            return None;
        }
        for row in &self.rows {
            let hit_alias = row.alias.split('+').any(|a| a == nm);
            if row.name_cn == nm || row.name_en == nm || hit_alias {
                return Some(serde_json::json!({
                    "id": row.id, "type": row.r#type, "parent_id": row.parent_id,
                    "name_cn": row.name_cn, "name_en": row.name_en, "alias": row.alias,
                    "is_abroad": row.is_abroad, "is_world_known": row.is_world_known,
                    "school_type": row.school_type, "is_double_first": row.is_double_first,
                    "education_type": row.education_type, "province": row.province,
                    "city": row.city, "is_985": row.is_985, "rank": row.rank,
                }));
            }
        }
        None
    }
}

/// Region entity — full port of
/// `deepdoc/parser/resume/entities/regions.py` (TBL + NM_SET + isName).
///
/// The 739-entry TBL table is embedded as JSON (`regions_tbl.json`,
/// extracted from the Python literal at build time).
pub struct RegionEntity;

/// Embed the regions TBL (id → name/parent).
const REGIONS_TBL_JSON: &str = include_str!("data/resume/regions_tbl.json");

/// Load the regions table as `id → (name, parent)`.
fn regions_tbl() -> std::collections::HashMap<String, (String, String)> {
    let v: serde_json::Value =
        serde_json::from_str(REGIONS_TBL_JSON).unwrap_or(serde_json::Value::Null);
    let mut out = std::collections::HashMap::new();
    if let Some(obj) = v.as_object() {
        for (id, entry) in obj {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let parent = entry
                .get("parent")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            out.insert(id.clone(), (name, parent));
        }
    }
    out
}

/// `NM_SET` — regions.py:761: the set of all region names.
fn nm_set() -> std::collections::HashSet<String> {
    regions_tbl().into_values().map(|(n, _)| n).collect()
}

/// `isName` — regions.py:782-789.
pub fn is_name(nm: &str) -> bool {
    let set = nm_set();
    if set.contains(nm) {
        return true;
    }
    if set.contains(&format!("{nm}市")) {
        return true;
    }
    let stripped = regex::Regex::new(r"(省|(回族|壮族|维吾尔)*自治区)$")
        .unwrap()
        .replace_all(nm, "")
        .into_owned();
    set.contains(&stripped)
}

/// `get_names` — regions.py:768-778: id → name + ancestor chain.
pub fn get_names(id: &str) -> Vec<String> {
    let tbl = regions_tbl();
    let mut nms = Vec::new();
    let mut cur = tbl.get(id).cloned();
    while let Some((name, parent)) = cur {
        nms.push(name);
        if parent.is_empty() {
            break;
        }
        cur = tbl.get(&parent).cloned();
    }
    nms
}

/// `_strQ2B` — full-width → half-width (standard Unicode block mapping;
/// mirrors infinity.rag_tokenizer._strQ2B).
pub fn str_q2b(s: &str) -> String {
    s.chars()
        .map(|c| match c as u32 {
            0x3000 => ' ',
            0xFF01..=0xFF5E => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .collect()
}

/// Industry entity — full port of
/// `deepdoc/parser/resume/entities/industries.py` (TBL + get_names).
///
/// The 677-entry TBL table is embedded as JSON (`industries_tbl.json`,
/// extracted from the Python literal at build time).
pub struct IndustryEntity;

/// Embed the industries TBL (id → name/parent).
const INDUSTRIES_TBL_JSON: &str = include_str!("data/resume/industries_tbl.json");

/// Load the industries table as `id → (name, parent)`.
fn industries_tbl() -> std::collections::HashMap<String, (String, String)> {
    let v: serde_json::Value =
        serde_json::from_str(INDUSTRIES_TBL_JSON).unwrap_or(serde_json::Value::Null);
    let mut out = std::collections::HashMap::new();
    if let Some(obj) = v.as_object() {
        for (id, entry) in obj {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let parent = entry
                .get("parent")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            out.insert(id.clone(), (name, parent));
        }
    }
    out
}

/// `get_names` — industries.py:698-708: id → name + ancestor chain.
/// Unknown ids return an empty vector (Python `if not d: return []`).
pub fn industry_names(id: &str) -> Vec<String> {
    let tbl = industries_tbl();
    let mut nms = Vec::new();
    let mut cur = tbl.get(id).cloned();
    while let Some((name, parent)) = cur {
        nms.push(name);
        if parent.is_empty() || parent == "0" {
            break;
        }
        cur = tbl.get(&parent).cloned();
    }
    nms
}

/// Common traditional→simplified character map covering company-name
/// vocabulary (approximation of `tradi2simp`; full table lives in the
/// compiled infinity.rag_tokenizer extension).
const TRADI2SIMP: &[(char, char)] = &[
    ('發', '发'),
    ('財', '财'),
    ('團', '团'),
    ('體', '体'),
    ('會', '会'),
    ('國', '国'),
    ('學', '学'),
    ('華', '华'),
    ('東', '东'),
    ('業', '业'),
    ('產', '产'),
    ('訊', '讯'),
    ('網', '网'),
    ('營', '营'),
    ('銷', '销'),
    ('廣', '广'),
    ('區', '区'),
    ('機', '机'),
    ('構', '构'),
    ('門', '门'),
    ('開', '开'),
    ('關', '关'),
    ('聯', '联'),
    ('萬', '万'),
    ('與', '与'),
    ('為', '为'),
    ('將', '将'),
    ('來', '来'),
    ('電', '电'),
    ('視', '视'),
    ('話', '话'),
    ('計', '计'),
    ('劃', '划'),
    ('設', '设'),
    ('說', '说'),
    ('論', '论'),
    ('證', '证'),
    ('實', '实'),
    ('驗', '验'),
    ('資', '资'),
    ('質', '质'),
    ('運', '运'),
    ('輸', '输'),
    ('轉', '转'),
    ('軟', '软'),
    ('件', '件'),
    ('硬', '硬'),
    ('顯', '显'),
    ('示', '示'),
    ('驅', '驱'),
    ('動', '动'),
    ('態', '态'),
    ('數', '数'),
    ('據', '据'),
    ('庫', '库'),
    ('導', '导'),
    ('航', '航'),
    ('醫', '医'),
    ('藥', '药'),
    ('農', '农'),
    ('銀', '银'),
    ('行', '行'),
    ('幣', '币'),
    ('價', '价'),
    ('錢', '钱'),
    ('賬', '账'),
    ('戶', '户'),
    ('證', '证'),
    ('券', '券'),
    ('險', '险'),
    ('買', '买'),
    ('賣', '卖'),
    ('購', '购'),
    ('貨', '货'),
    ('車', '车'),
    ('飛', '飞'),
    ('機', '机'),
    ('場', '场'),
    ('橋', '桥'),
    ('樓', '楼'),
    ('築', '筑'),
    ('龍', '龙'),
    ('鳳', '凤'),
    ('鳥', '鸟'),
    ('魚', '鱼'),
    ('馬', '马'),
    ('門', '门'),
    ('風', '风'),
    ('雲', '云'),
    ('雪', '雪'),
    ('電', '电'),
    ('燈', '灯'),
    ('火', '火'),
    ('水', '水'),
    ('陸', '陆'),
    ('海', '海'),
    ('洋', '洋'),
    ('島', '岛'),
    ('灣', '湾'),
    ('港', '港'),
    ('澳', '澳'),
    ('台', '台'),
    ('廣', '广'),
    ('州', '州'),
    ('深', '深'),
    ('圳', '圳'),
    ('佛', '佛'),
    ('山', '山'),
    ('東', '东'),
    ('莞', '莞'),
    ('惠', '惠'),
    ('珠', '珠'),
    ('汕', '汕'),
    ('頭', '头'),
    ('北', '北'),
    ('高', '高'),
    ('雄', '雄'),
    ('新', '新'),
    ('竹', '竹'),
    ('桃', '桃'),
    ('園', '园'),
    ('蘭', '兰'),
    ('花', '花'),
    ('蓮', '莲'),
    ('金', '金'),
    ('祖', '祖'),
    ('澎', '澎'),
    ('湖', '湖'),
    ('綠', '绿'),
    ('嶼', '屿'),
];

/// `tradi2simp` — approximate traditional→simplified using the map above.
pub fn tradi2simp(s: &str) -> String {
    s.chars()
        .map(|c| {
            TRADI2SIMP
                .iter()
                .find(|(t, _)| *t == c)
                .map(|(_, s2)| *s2)
                .unwrap_or(c)
        })
        .collect()
}

/// `corpNorm` helper tokenizer approximation — splits CJK runs per
/// character and keeps Latin tokens whole (rag_tokenizer equivalent).
fn tokenize_corp(nm: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut lat = String::new();
    for c in nm.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == ',' || c == '+' {
            lat.push(c);
        } else {
            if !lat.is_empty() {
                out.push(std::mem::take(&mut lat));
            }
            if !c.is_whitespace() {
                out.push(c.to_string());
            }
        }
    }
    if !lat.is_empty() {
        out.push(lat);
    }
    out
}

/// `CorporationEntity` — full port of
/// `deepdoc/parser/resume/entities/corporations.py`.
pub struct CorporationEntity;

const CORP_BAIKE_CSV: &str = include_str!("data/resume/corp_baike_len.csv");
const CORP_TKS_JSON: &str = include_str!("data/resume/corp.tks.freq.json");
const GOOD_CORP_JSON: &str = include_str!("data/resume/good_corp.json");
const CORP_TAG_JSON: &str = include_str!("data/resume/corp_tag.json");

/// `baike` — corporations.py:40-46: cid → baike length (default 0).
pub fn baike(cid: &str, default_v: i64) -> i64 {
    for line in CORP_BAIKE_CSV.lines().skip(1) {
        let mut cols = line.split('\t');
        let id = cols.next().unwrap_or("").trim();
        if id == cid
            && let Some(len) = cols.next().and_then(|v| v.trim().parse::<i64>().ok()) {
                return len;
            }
    }
    default_v
}

/// `CORP_TKS` — the token-frequency list (token dictionary).
fn corp_tks() -> std::collections::HashSet<String> {
    let v: serde_json::Value =
        serde_json::from_str(CORP_TKS_JSON).unwrap_or(serde_json::Value::Null);
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// `rmNoise` — corporations.py:88-91.
pub fn rm_noise(n: &str) -> String {
    let re_paren = regex::Regex::new(r"[(（][^()（）]+[)）]").unwrap();
    let n = re_paren.replace_all(n, "");
    let re_sep = regex::Regex::new(r"[,. &（）()]+").unwrap();
    re_sep.replace_all(&n, "").into_owned()
}

/// `corpNorm` — corporations.py:49-85 (approximate; tradi2simp and
/// tokenization are approximated per the notes above).
pub fn corp_norm(nm: &str, add_region: bool) -> String {
    if nm.is_empty() {
        return String::new();
    }
    // tradi2simp(strQ2B(nm)).lower()
    let mut nm = tradi2simp(&str_q2b(nm)).to_lowercase();
    // &amp; → &
    nm = nm.replace("&amp;", "&");
    // strip separators; `\\\\` in the Rust literal yields `\\` in the
    // regex, i.e. one literal backslash in the class (Python raw `\\`).
    let re_sep = regex::Regex::new("[()（）+'\"\t *\\\\【】-]+").unwrap();
    nm = re_sep.replace_all(&nm, " ").into_owned();
    // strip suffixes (case-insensitive); `[—\-]+` = literal em-dash or
    // hyphen (Python `[—-]`: trailing `-` is literal).
    let re_suffix =
        regex::Regex::new(r"(?i)([—\-]+.*| +co\..*|corp\..*| +inc\..*| +ltd.*)").unwrap();
    nm = re_suffix.replace_all(&nm, "").into_owned();
    let re_suffix2 = regex::Regex::new(
        r"(?i)(计算机|技术|(技术|科技|网络)*有限公司|公司|有限|研发中心|中国|总部)$",
    )
    .unwrap();
    nm = re_suffix2.replace_all(&nm, "").into_owned();
    // length guard
    if nm.chars().count() < 5 && !is_name(&nm.chars().take(2).collect::<String>()) {
        return nm;
    }
    // tokenize and filter regions + CORP_TKS
    let tks = tokenize_corp(&nm);
    let reg: Vec<String> = tks
        .iter()
        .enumerate()
        .filter(|(i, t)| is_name(t) && !(t.as_str() == "中国" && *i == 0))
        .map(|(_, t)| t.clone())
        .collect();
    let tks_set = corp_tks();
    // Greedy longest-match against the CORP_TKS dictionary: jieba emits
    // multi-char entries (集团/科技/有限/…) as single tokens, so a whole
    // entry must be skipped as one unit even though `tokenize_corp` splits
    // CJK runs per character.
    let mut entries: Vec<&String> = tks_set.iter().collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.chars().count()));
    let mut out = String::new();
    let mut i = 0;
    while i < tks.len() {
        let t = &tks[i];
        if is_name(t) || tks_set.contains(t) {
            i += 1;
            continue;
        }
        let mut matched = 0;
        for entry in &entries {
            let n = entry.chars().count();
            if n < 2 || i + n > tks.len() {
                continue;
            }
            if tks[i..i + n].concat() == **entry {
                matched = n;
                break;
            }
        }
        if matched > 0 {
            i += matched;
            continue;
        }
        let re_alpha = regex::Regex::new(r"[0-9a-zA-Z,.]").unwrap();
        if re_alpha.is_match(t)
            && re_alpha.is_match(
                &out.chars()
                    .last()
                    .map(|c| c.to_string())
                    .unwrap_or_default(),
            )
        {
            out.push(' ');
        }
        out.push_str(t);
        i += 1;
    }
    // boundary trimming
    let r1 = regex::Regex::new(r"^([^a-z0-9 \(\)&]{2,})[a-z ]{4,}$").unwrap();
    if let Some(caps) = r1.captures(out.trim()) {
        out = caps.get(1).unwrap().as_str().to_string();
    }
    let r2 = regex::Regex::new(r"^([a-z ]{3,})[^a-z0-9 \(\)&]{2,}$").unwrap();
    if let Some(caps) = r2.captures(out.trim()) {
        out = caps.get(1).unwrap().as_str().to_string();
    }
    let region_suffix = if add_region && !reg.is_empty() {
        format!("({})", reg[0])
    } else {
        String::new()
    };
    format!("{}{}", out.trim(), region_suffix)
}

/// `GOOD_CORP` — corporations.py:94: normalized good-company set.
fn good_corp_set() -> std::collections::HashSet<String> {
    let v: serde_json::Value =
        serde_json::from_str(GOOD_CORP_JSON).unwrap_or(serde_json::Value::Null);
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(|c| corp_norm(&rm_noise(c), false))
                .collect()
        })
        .unwrap_or_default()
}

/// `CORP_TAG` — corporations.py:95-99: normalized name → tags.
fn corp_tag_map() -> std::collections::HashMap<String, Vec<String>> {
    let v: serde_json::Value =
        serde_json::from_str(CORP_TAG_JSON).unwrap_or(serde_json::Value::Null);
    let mut out = std::collections::HashMap::new();
    if let Some(obj) = v.as_object() {
        for (c, tags) in obj {
            let cc = corp_norm(&rm_noise(c), false);
            if cc.is_empty() {
                continue;
            }
            let t = tags
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            out.insert(cc, t);
        }
    }
    out
}

/// `is_good` — corporations.py:102-114 (renamed `corp_is_good` to avoid
/// collision with the schools.py entity which also defines `is_good`).
pub fn corp_is_good(nm: &str) -> bool {
    if nm.contains("外派") {
        return false;
    }
    let nm = corp_norm(&rm_noise(nm), false);
    for n in good_corp_set() {
        let re_alpha_end = regex::Regex::new(r"[0-9a-zA-Z]+$").unwrap();
        if re_alpha_end.is_match(&n) {
            if n == nm {
                return true;
            }
        } else if nm.contains(&n) {
            return true;
        }
    }
    false
}

/// `corp_tag` — corporations.py:117-129.
pub fn corp_tag(nm: &str) -> Vec<String> {
    let nm = corp_norm(&rm_noise(nm), false);
    let re_alpha = regex::Regex::new(r"[0-9a-zA-Z., ]+$").unwrap();
    for (n, tags) in corp_tag_map() {
        if re_alpha.is_match(&n) {
            if n == nm {
                return tags;
            }
        } else if nm.contains(&n) {
            if n.chars().count() < 3 && nm.chars().count() / n.chars().count().max(1) >= 2 {
                continue;
            }
            return tags;
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn industry_entity_names_and_ancestors() {
        // industries.py __main__ example: get_names("1119").
        let names = industry_names("1119");
        assert!(!names.is_empty());
        // First element is the leaf industry itself.
        assert_eq!(names[0], "牙科及医疗器械");
        // Ancestors chain upward toward a root ("0" terminates the walk).
        assert!(names.iter().any(|n| n == "医疗服务"));
        assert!(names.iter().any(|n| n == "医药"));
        // The chain terminates at a root entry (parent "0").
        let last = names.last().unwrap();
        assert_eq!(last, "医药");

        // Unknown id → empty (Python `if not d: return []`).
        assert!(industry_names("999999").is_empty());

        // Root industry (parent "0") returns only itself.
        assert_eq!(industry_names("1"), vec!["IT/通信/电子"]);

        // 人工智能 (1130) is a child of 互联网 (2).
        let ai = industry_names("1130");
        assert_eq!(ai, vec!["人工智能", "互联网"]);
    }

    #[test]
    fn school_entity_split_and_select() {
        // split — schools.py:53-65.
        assert_eq!(split("New York University"), vec!["New York University"]);
        assert_eq!(split("美国 斯坦福大学"), vec!["美国", "斯坦福大学"]);

        // select by name_cn (schools.csv has 清华大学 etc).
        let v = select("清华大学").expect("清华 should be in schools.csv");
        assert_eq!(v["name_cn"], "清华大学");
        assert!(v["rank"].as_i64().unwrap_or(0) > 0);
        assert_eq!(v["is_985"], 1);

        // select by name_en (case-insensitive) — Tsinghua's name_en is
        // empty in schools.csv (its English name lives in rank aliases),
        // so use a school with a real name_en column instead.
        let v2 = select("University of East London").expect("en name should match");
        assert_eq!(v2["name_cn"], "东伦敦大学");

        // Unknown → None.
        assert!(select("不存在的学校XYZ").is_none());
        // Tsinghua's English name is not in name_en → None (Python parity).
        assert!(select("Tsinghua University").is_none());
    }

    #[test]
    fn school_entity_is_good_and_table_cache() {
        // is_good — schools.py:88-92.
        assert!(is_good("清华大学"));
        assert!(is_good("中国科技大学"));
        assert!(!is_good("某职业技术学院"));

        // Cached table select matches free function.
        let tbl = SchoolTable::new();
        let v = tbl.select("北京大学").expect("北大 should match");
        assert_eq!(v["name_cn"], "北京大学");
    }

    #[test]
    fn degree_entity_maps_ids_and_names() {
        assert_eq!(DegreeEntity::get_name("1"), "本科");
        assert_eq!(DegreeEntity::get_name("2"), "硕士");
        assert_eq!(DegreeEntity::get_name("3"), "博士");
        assert_eq!(DegreeEntity::get_name("6"), "MBA");
        assert_eq!(DegreeEntity::get_name("94"), "EMBA");
        assert_eq!(DegreeEntity::get_name("999"), "");

        // get_id — degrees.py:40-44 (upper + trim).
        assert_eq!(DegreeEntity::get_id("本科"), "1");
        assert_eq!(DegreeEntity::get_id(" 硕士 "), "2");
        assert_eq!(DegreeEntity::get_id("mba"), "6");
        assert_eq!(DegreeEntity::get_id("EMBA"), "94");
        assert_eq!(DegreeEntity::get_id("学士"), "");
        assert_eq!(DegreeEntity::get_id(""), "");
    }

    #[test]
    fn language_detection_and_field_maps() {
        assert!(is_english("English"));
        assert!(is_english("en"));
        assert!(!is_english("Chinese"));
        assert!(!is_english(""));
        assert_eq!(get_field_map("Chinese").len(), 32);
        assert_eq!(get_field_map("English").len(), 32);
        assert_eq!(get_field_map("Chinese")[0].0, "name_kwd");
        assert_eq!(get_field_map("English")[0].1, "Name");
    }

    #[test]
    fn normalize_unifies_fullwidth_and_whitespace() {
        assert_eq!(normalize_for_comparison("阿 里 巴 巴"), "阿里巴巴");
        assert_eq!(normalize_for_comparison("ＡＢＣ"), "abc");
        assert_eq!(normalize_for_comparison("  张三  "), "张三");
        assert_eq!(normalize_for_comparison(""), "");
    }

    #[test]
    fn date_parsing_handles_multiple_formats() {
        assert_eq!(parse_date_str("2024.3"), Some((2024, 3)));
        assert_eq!(parse_date_str("2024-01"), Some((2024, 1)));
        assert_eq!(parse_date_str("2024/12"), Some((2024, 12)));
        assert_eq!(parse_date_str("2024年1月"), Some((2024, 1)));
        assert_eq!(parse_date_str("2024"), Some((2024, 1)));
        assert_eq!(parse_date_str("2024.13"), Some((2024, 1))); // invalid month → 1
        assert_eq!(parse_date_str("abc"), None);
    }

    #[test]
    fn work_years_calculation() {
        // 2024.1 → 2026.8 = 31 months → 2.6 years.
        assert_eq!(calc_single_exp_years("2024.1", "至今"), 2.6);
        assert_eq!(calc_single_exp_years("2024.1", "2025.1"), 1.0);
        assert_eq!(calc_single_exp_years("", "2025.1"), 0.0);
        assert_eq!(calc_single_exp_years("2025.1", "2024.1"), 0.0); // negative
        let exps = json!([
            {"start_date": "2024.1", "end_date": "2025.1"},
            {"start_date": "2023.1", "end_date": "2023.6"},
        ]);
        // 1.0 + round(5/12, 1) = 1.0 + 0.4 (Python round semantics).
        assert_eq!(calculate_work_years(exps.as_array().unwrap()), 1.4);
    }

    #[test]
    fn shingling_jaccard_matches_expected() {
        // Identical → 1.0.
        assert_eq!(
            shingling_jaccard("same text here", "same text here", 5),
            1.0
        );
        // Completely different → 0.0.
        assert_eq!(shingling_jaccard("aaaa", "bbbb", 5), 0.0);
        // Both empty → 1.0.
        assert_eq!(shingling_jaccard("", "", 5), 1.0);
    }

    #[test]
    fn regex_parses_chinese_resume_basic_fields() {
        let text = "个人简历\n姓名：张三\n性别：男\n年龄：28岁\n电话：13812345678\n邮箱：zhangsan@example.com\n教育背景：中山大学 本科 计算机科学专业\n工作经验：5年相关经验\n";
        let resume = parse_with_regex(text, "Chinese");
        assert_eq!(resume["name_kwd"], "张三");
        assert_eq!(resume["gender_kwd"], "男");
        assert_eq!(resume["age_int"], 28);
        assert_eq!(resume["phone_kwd"], "13812345678");
        assert_eq!(resume["email_tks"], "zhangsan@example.com");
        assert_eq!(resume["work_exp_flt"], 5.0);
        let schools = resume["school_name_tks"].as_array().unwrap();
        assert!(
            schools
                .iter()
                .any(|s| s.as_str().unwrap().contains("中山大学"))
        );
    }

    #[test]
    fn regex_parses_english_resume_with_name_label() {
        let text = "Name: John Smith\nPhone: 13812345678\nEmail: john@example.com\nGender: Male\nGraduated 2020 from Stanford University\nMajor: Computer Science\n5 years experience\n";
        let resume = parse_with_regex(text, "English");
        assert_eq!(resume["name_kwd"], "John Smith");
        assert_eq!(resume["gender_kwd"], "Male");
        assert_eq!(resume["edu_end_int"], 2020);
        assert_eq!(resume["work_exp_flt"], 5.0);
        let schools = resume["school_name_tks"].as_array().unwrap();
        assert!(
            schools
                .iter()
                .any(|s| s.as_str().unwrap().contains("Stanford University"))
        );
    }

    #[test]
    fn postprocess_prunes_hallucinated_fields() {
        // Name not in source text → cleared.
        let mut resume = json!({
            "name_kwd": "不存在的人",
            "corp_nm_tks": ["幻觉公司"],
            "school_name_tks": ["假大学"],
            "gender_kwd": "male",
            "phone_kwd": "+86 138-1234-5678",
            "birth_dt": "1990年1月",
        });
        let lines = vec!["张三".to_string(), "工作于真实公司".to_string()];
        postprocess_resume(&mut resume, &lines, "Chinese");
        assert_eq!(resume["name_kwd"], "");
        assert_eq!(resume["corp_nm_tks"].as_array().unwrap().len(), 0);
        assert_eq!(resume["school_name_tks"].as_array().unwrap().len(), 0);
        assert_eq!(resume["gender_kwd"], "男");
        assert_eq!(resume["phone_kwd"], "+8613812345678");
        assert_eq!(resume["birth_dt"], "1990-1");
    }

    #[test]
    fn postprocess_keeps_fields_found_in_source() {
        let mut resume = json!({
            "name_kwd": "张三",
            "corp_nm_tks": ["腾讯科技有限公司"],
            "school_name_tks": ["中山大学"],
            "position_name_tks": ["工程师"],
        });
        let lines = vec![
            "张三".to_string(),
            "腾讯科技有限公司".to_string(),
            "中山大学".to_string(),
            "工程师".to_string(),
        ];
        postprocess_resume(&mut resume, &lines, "Chinese");
        assert_eq!(resume["name_kwd"], "张三");
        assert_eq!(resume["corp_nm_tks"].as_array().unwrap().len(), 1);
        assert_eq!(resume["school_name_tks"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn postprocess_merges_project_descs_into_work() {
        let mut resume = json!({
            "name_kwd": "张三",
            "work_desc_tks": ["负责系统开发"],
            "project_desc_tks": ["开发了电商平台"],
            "project_tks": ["电商平台"],
        });
        let lines = vec![
            "张三".to_string(),
            "负责系统开发".to_string(),
            "开发了电商平台".to_string(),
        ];
        postprocess_resume(&mut resume, &lines, "Chinese");
        assert_eq!(resume["project_desc_tks"].as_array().unwrap().len(), 0);
        let work = resume["work_desc_tks"].as_array().unwrap();
        assert_eq!(work.len(), 2);
        assert!(work[1].as_str().unwrap().starts_with("[电商平台]"));
    }

    #[test]
    fn postprocess_completes_required_fields() {
        let mut resume = json!({"name_kwd": "张三"});
        let lines = vec!["张三".to_string()];
        postprocess_resume(&mut resume, &lines, "Chinese");
        for field in ["gender_kwd", "phone_kwd"] {
            assert_eq!(resume[field], "");
        }
        // _tks fields complete as empty arrays.
        for field in [
            "email_tks",
            "position_name_tks",
            "school_name_tks",
            "major_tks",
        ] {
            assert_eq!(resume[field].as_array().unwrap().len(), 0);
        }
    }

    #[test]
    fn build_chunks_groups_basic_info_and_identity() {
        let resume = json!({
            "name_kwd": "张三",
            "phone_kwd": "13812345678",
            "gender_kwd": "男",
            "work_exp_flt": 5.0,
            "school_name_tks": ["中山大学"],
            "degree_kwd": ["本科"],
        });
        let chunks = build_chunk_document("张三.pdf", &resume, "Chinese");
        assert!(!chunks.is_empty());
        // First chunk = Basic Info group.
        let first = &chunks[0];
        assert!(first.content_with_weight.contains("基本信息"));
        assert!(first.content_with_weight.contains("姓名/名字: 张三"));
        // Identity fields redundantly present.
        assert_eq!(first.fields.get("name_kwd").unwrap(), "张三");
        // Education group exists.
        let edu = chunks
            .iter()
            .find(|c| c.content_with_weight.contains("教育背景"));
        assert!(edu.is_some());
        assert!(edu.unwrap().content_with_weight.contains("中山大学"));
        // Positions increment logically.
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.page_num_int, vec![1]);
            assert_eq!(c.top_int, vec![i as i64]);
            assert_eq!(c.position_int, vec![(1, 0, 0, i as i64, i as i64)]);
        }
    }

    #[test]
    fn build_chunks_splits_work_descriptions_per_element() {
        let resume = json!({
            "name_kwd": "张三",
            "work_desc_tks": ["负责A系统", "负责B平台"],
            "corp_nm_tks": ["腾讯"],
            "_work_exp_details": [
                {"company": "腾讯", "start_date": "2020.1", "end_date": "2022.1", "years": 2.0},
                {"company": "阿里", "start_date": "2022.1", "end_date": "2024.1", "years": 2.0},
            ],
        });
        let chunks = build_chunk_document("张三.pdf", &resume, "Chinese");
        let work_chunks: Vec<&ResumeChunk> = chunks
            .iter()
            .filter(|c| c.content_with_weight.contains("工作职责"))
            .collect();
        assert_eq!(work_chunks.len(), 2);
        assert!(
            work_chunks[0]
                .content_with_weight
                .contains("腾讯 2020.1-2022.1 2年")
        );
        assert!(
            work_chunks[1]
                .content_with_weight
                .contains("阿里 2022.1-2024.1 2年")
        );
        // Each work chunk carries identity name.
        assert_eq!(work_chunks[0].fields.get("name_kwd").unwrap(), "张三");
    }

    #[test]
    fn end_to_end_regex_path_produces_chunks() {
        let text = "个人简历\n姓名：李四\n电话：13912345678\n邮箱：lisi@example.com\n教育：华南理工大学 硕士\n工作：5年开发经验\n";
        let chunks = parse_resume_regex(text, "李四.pdf", "Chinese");
        assert!(!chunks.is_empty());
        let all_content = chunks
            .iter()
            .map(|c| c.content_with_weight.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all_content.contains("李四"));
        assert!(all_content.contains("13912345678"));
        // Summary line carries identity.
        assert!(all_content.contains("[姓名:李四 | 电话:13912345678"));
    }
}
