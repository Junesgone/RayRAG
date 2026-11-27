//! Chinese surname detection — mirrors RAGFlow `rag/nlp/surname.py`.
//!
//! The upstream file is a 144-line module whose body is a single set `m` of
//! ~500 single-character surnames (百家姓) plus compound surnames, and one
//! predicate `isit(n)` = `n.strip() in m`. RAGFlow uses it in
//! `deepdoc/parser/resume/step_two.py` to verify that a candidate line is a
//! Chinese personal name (single-char surname, or two-char compound surname).
//!
//! We embed the same set verbatim and expose `is_chinese_surname`, which
//! checks the full name against single-char surnames and the leading one/two
//! characters against compound surnames — matching the resume usage pattern
//! `surname.isit(nm[0]) or surname.isit(nm[:2])`.

/// All single-character Chinese surnames (RAGFlow surname.py `m` set).
const SINGLE_SURNAMES: &[&str] = &[
    "赵", "钱", "孙", "李", "周", "吴", "郑", "王", "冯", "陈", "褚", "卫", "蒋", "沈", "韩", "杨",
    "朱", "秦", "尤", "许", "何", "吕", "施", "张", "孔", "曹", "严", "华", "金", "魏", "陶", "姜",
    "戚", "谢", "邹", "喻", "柏", "水", "窦", "章", "云", "苏", "潘", "葛", "奚", "范", "彭", "郎",
    "鲁", "韦", "昌", "马", "苗", "凤", "花", "方", "俞", "任", "袁", "柳", "酆", "鲍", "史", "唐",
    "费", "廉", "岑", "薛", "雷", "贺", "倪", "汤", "滕", "殷", "罗", "毕", "郝", "邬", "安", "常",
    "乐", "于", "时", "傅", "皮", "卞", "齐", "康", "伍", "余", "元", "卜", "顾", "孟", "平", "黄",
    "和", "穆", "萧", "尹", "姚", "邵", "湛", "汪", "祁", "毛", "禹", "狄", "米", "贝", "明", "臧",
    "计", "伏", "成", "戴", "谈", "宋", "茅", "庞", "熊", "纪", "舒", "屈", "项", "祝", "董", "梁",
    "杜", "阮", "蓝", "闵", "席", "季", "麻", "强", "贾", "路", "娄", "危", "江", "童", "颜", "郭",
    "梅", "盛", "林", "刁", "钟", "徐", "邱", "骆", "高", "夏", "蔡", "田", "樊", "胡", "凌", "霍",
    "虞", "万", "支", "柯", "昝", "管", "卢", "莫", "经", "房", "裘", "缪", "干", "解", "应", "宗",
    "丁", "宣", "贲", "邓", "郁", "单", "杭", "洪", "包", "诸", "左", "石", "崔", "吉", "钮", "龚",
    "程", "嵇", "邢", "滑", "裴", "陆", "荣", "翁", "荀", "羊", "於", "惠", "甄", "曲", "家", "封",
    "芮", "羿", "储", "靳", "汲", "邴", "糜", "松", "井", "段", "富", "巫", "乌", "焦", "巴", "弓",
    "牧", "隗", "山", "谷", "车", "侯", "宓", "蓬", "全", "郗", "班", "仰", "秋", "仲", "伊", "宫",
    "宁", "仇", "栾", "暴", "甘", "钭", "厉", "戎", "祖", "武", "符", "刘", "景", "詹", "束", "龙",
    "叶", "幸", "司", "韶", "郜", "黎", "蓟", "薄", "印", "宿", "白", "怀", "蒲", "邰", "从", "鄂",
    "索", "咸", "籍", "赖", "卓", "蔺", "屠", "蒙", "池", "乔", "阴", "鬱", "胥", "能", "苍", "双",
    "闻", "莘", "党", "翟", "谭", "贡", "劳", "逄", "姬", "申", "扶", "堵", "冉", "宰", "郦", "雍",
    "郤", "璩", "桑", "桂", "濮", "牛", "寿", "通", "边", "扈", "燕", "冀", "郏", "浦", "尚", "农",
    "温", "别", "庄", "晏", "柴", "瞿", "阎", "充", "慕", "连", "茹", "习", "宦", "艾", "鱼", "容",
    "向", "古", "易", "慎", "戈", "廖", "庾", "终", "暨", "居", "衡", "步", "都", "耿", "满", "弘",
    "匡", "国", "文", "寇", "广", "禄", "阙", "东", "欧", "殳", "沃", "利", "蔚", "越", "夔", "隆",
    "师", "巩", "厍", "聂", "晁", "勾", "敖", "融", "冷", "訾", "辛", "阚", "那", "简", "饶", "空",
    "曾", "母", "沙", "乜", "养", "鞠", "须", "丰", "巢", "关", "蒯", "相", "查", "后", "荆", "红",
    "游", "竺", "权", "逯", "盖", "益", "桓", "公", "兰", "原", "乞", "西", "阿", "肖", "丑", "位",
    "曽", "巨", "德", "代", "圆", "尉", "仵", "纳", "仝", "脱", "丘", "但", "展", "迪", "付", "覃",
    "晗", "特", "隋", "苑", "奥", "漆", "谌", "郄", "练", "扎", "邝", "渠", "信", "门", "陳", "化",
    "密", "泮", "鹿", "赫", "晋", "楚", "闫", "法", "汝", "鄢", "涂", "钦", "呼延", "归", "海",
    "羊舌", "微", "生", "岳", "帅", "缑", "亢", "况", "后", "有", "琴", "商", "牟", "佘", "佴",
    "伯", "赏", "南宫", "墨", "哈", "谯", "笪", "年", "爱", "阳", "佟", "第五", "言", "福",
];

/// Compound (two-character) Chinese surnames — a separate check because
/// `surname.isit(nm[:2])` in resume parsing tests the leading bigram.
const COMPOUND_SURNAMES: &[&str] = &[
    "万俟", "司马", "上官", "欧阳", "夏侯", "诸葛", "闻人", "东方", "赫连", "皇甫", "尉迟", "公羊",
    "澹台", "公冶", "宗政", "濮阳", "淳于", "单于", "太叔", "申屠", "公孙", "仲孙", "轩辕", "令狐",
    "钟离", "宇文", "长孙", "慕容", "鲜于", "闾丘", "司徒", "司空", "亓官", "司寇", "仉督", "子车",
    "颛孙", "端木", "巫马", "公西", "漆雕", "乐正", "壤驷", "公良", "拓跋", "夹谷", "宰父", "榖梁",
    "段干", "百里", "东郭", "南门", "呼延", "梁丘", "左丘", "东门", "西门",
];

/// Whether `name` begins with a Chinese surname — mirrors RAGFlow
/// `surname.isit(nm[0]) or surname.isit(nm[:2])`.
///
/// A single-character surname matches directly; otherwise the leading two
/// characters are tested against the compound-surname table. Whitespace is
/// trimmed first (upstream `isit` strips the input).
pub fn is_chinese_surname(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    let chars: Vec<char> = name.chars().collect();
    let first: String = chars[0].to_string();
    if SINGLE_SURNAMES.contains(&first.as_str()) {
        return true;
    }
    if chars.len() >= 2 {
        let bigram: String = chars[..2].iter().collect();
        if COMPOUND_SURNAMES.contains(&bigram.as_str()) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_single_surnames_match() {
        for s in ["李", "王", "张", "刘", "陈", "杨", "赵", "黄", "周", "吴"] {
            assert!(is_chinese_surname(s), "{s} is a surname");
        }
    }

    #[test]
    fn full_names_match_on_first_char() {
        assert!(is_chinese_surname("李锦澎"));
        assert!(is_chinese_surname("王小明"));
        assert!(is_chinese_surname("欧阳娜娜"));
    }

    #[test]
    fn compound_surnames_match_bigram() {
        for s in [
            "欧阳", "司马", "上官", "诸葛", "慕容", "东方", "尉迟", "公孙",
        ] {
            assert!(is_chinese_surname(s), "{s} is a compound surname");
            assert!(is_chinese_surname(&format!("{s}某")), "{s}某 should match");
        }
    }

    #[test]
    fn non_surnames_do_not_match() {
        for s in ["啊", "哦", "呗", "哟", "哎", "嘻"] {
            assert!(!is_chinese_surname(s), "{s} is not a surname");
        }
    }

    #[test]
    fn whitespace_is_trimmed() {
        assert!(is_chinese_surname(" 李 "));
        assert!(!is_chinese_surname("  "));
        assert!(!is_chinese_surname(""));
    }

    #[test]
    fn surnames_in_ragflow_set_match() {
        // Spot-check rare entries from the upstream set (郁/湛/酆/璩/郄)
        for s in [
            "郁", "湛", "酆", "璩", "郄", "逯", "訾", "阚", "仉督", "澹台",
        ] {
            assert!(is_chinese_surname(s), "{s} should be in the 百家姓 set");
        }
    }
}
