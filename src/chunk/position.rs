//! RAGFlow PDF position-tag extraction.
//!
//! DeepDOC parsers append `@@page\tx0\tx1\ty0\ty1##` tags to section text.
//! The tags are not user-visible content: the chunking boundary removes them
//! and projects their coordinates into the index metadata fields consumed by
//! RAGFlow-compatible chunk and retrieval APIs.

use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct PositionMetadata {
    pub positions: Vec<[i32; 5]>,
    pub page_numbers: Vec<i32>,
    pub tops: Vec<i32>,
}

pub(super) fn extract(text: &str) -> PositionMetadata {
    static POSITION_TAG: OnceLock<Regex> = OnceLock::new();
    let regex = POSITION_TAG.get_or_init(|| {
        Regex::new(r"@@([0-9-]+)\t([0-9.]+)\t([0-9.]+)\t([0-9.]+)\t([0-9.]+)##")
            .expect("RAGFlow position regex is valid")
    });
    let mut metadata = PositionMetadata::default();
    for captures in regex.captures_iter(text) {
        let Some(left) = coordinate(&captures[2]) else {
            continue;
        };
        let Some(right) = coordinate(&captures[3]) else {
            continue;
        };
        let Some(top) = coordinate(&captures[4]) else {
            continue;
        };
        let Some(bottom) = coordinate(&captures[5]) else {
            continue;
        };
        for page_number in captures[1]
            .split('-')
            .filter_map(|page| page.parse::<i32>().ok())
        {
            metadata
                .positions
                .push([page_number, left, right, top, bottom]);
            metadata.page_numbers.push(page_number);
            metadata.tops.push(top);
        }
    }
    metadata
}

pub(super) fn remove(text: &str) -> String {
    static ANY_POSITION_TAG: OnceLock<Regex> = OnceLock::new();
    ANY_POSITION_TAG
        .get_or_init(|| {
            Regex::new(r"@@[\t0-9.\-]+?##").expect("RAGFlow position-removal regex is valid")
        })
        .replace_all(text, "")
        .trim()
        .to_owned()
}

fn coordinate(value: &str) -> Option<i32> {
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(|value| value as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_matches_ragflow_page_and_coordinate_projection() {
        let text = concat!(
            "one@@1\t10.9\t100.2\t20.8\t40.7##",
            "two@@2-3\t50.0\t150.0\t60.0\t90.0##"
        );
        let metadata = extract(text);
        assert_eq!(
            metadata.positions,
            [
                [1, 10, 100, 20, 40],
                [2, 50, 150, 60, 90],
                [3, 50, 150, 60, 90]
            ]
        );
        assert_eq!(metadata.page_numbers, [1, 2, 3]);
        assert_eq!(metadata.tops, [20, 60, 60]);
    }

    #[test]
    fn removal_accepts_negative_coordinates_that_upstream_does_not_extract() {
        let text = "visible@@1\t-10.0\t100.0\t-2.0\t40.0##";
        assert!(extract(text).positions.is_empty());
        assert_eq!(remove(text), "visible");
    }
}
