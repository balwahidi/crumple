//! Final candidate choice.

/// One encoded candidate for an image.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// Opaque codec id.
    pub id: u8,
    pub bytes: u64,
    pub passes: bool,
}

/// Smallest passing candidate strictly smaller than `original_bytes`; ties go to the lower id. None = keep the original.
pub fn choose(original_bytes: u64, candidates: &[Candidate]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.passes && c.bytes < original_bytes)
        .min_by_key(|(_, c)| (c.bytes, c.id))
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: u8, bytes: u64, passes: bool) -> Candidate {
        Candidate { id, bytes, passes }
    }

    #[test]
    fn smallest_wins() {
        assert_eq!(
            choose(100, &[c(0, 80, true), c(1, 50, true), c(2, 60, true)]),
            Some(1)
        );
    }

    #[test]
    fn ties_go_to_lower_id() {
        assert_eq!(choose(100, &[c(2, 50, true), c(1, 50, true)]), Some(1));
    }

    #[test]
    fn none_when_not_smaller() {
        assert_eq!(choose(100, &[c(0, 100, true), c(1, 150, true)]), None);
        assert_eq!(choose(100, &[]), None);
    }

    #[test]
    fn failing_ignored() {
        assert_eq!(choose(100, &[c(0, 10, false), c(1, 90, true)]), Some(1));
        assert_eq!(choose(100, &[c(0, 10, false)]), None);
    }
}
