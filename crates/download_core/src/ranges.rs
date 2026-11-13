use crate::{ByteRange, Error, Result};

pub(crate) fn validate(ranges: &[ByteRange], total: u64) -> Result<()> {
    let mut previous = 0;
    for (index, range) in ranges.iter().enumerate() {
        if range.is_empty() || range.end > total || (index > 0 && range.start < previous) {
            return Err(Error::Storage(
                "Invalid or overlapping persisted byte ranges".into(),
            ));
        }
        previous = range.end;
    }
    Ok(())
}
pub(crate) fn bytes(ranges: &[ByteRange]) -> u64 {
    ranges.iter().map(ByteRange::len).sum()
}
pub(crate) fn complete(ranges: &mut Vec<ByteRange>, range: ByteRange) {
    ranges.push(range);
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<ByteRange> = Vec::with_capacity(ranges.len());
    for range in ranges.drain(..) {
        if let Some(last) = merged.last_mut()
            && last.end == range.start
        {
            last.end = range.end;
        } else {
            merged.push(range);
        }
    }
    *ranges = merged;
}
/// Allocate the next gap lazily; at most one interval per running worker is reserved.
pub(crate) fn next(
    completed: &[ByteRange],
    active: &[ByteRange],
    total: u64,
    size: u64,
) -> Option<ByteRange> {
    let mut occupied: Vec<_> = completed.iter().chain(active).copied().collect();
    occupied.sort_by_key(|range| range.start);
    let mut cursor = 0;
    for range in occupied {
        if cursor < range.start {
            return Some(ByteRange {
                start: cursor,
                end: range.start.min(cursor.saturating_add(size)),
            });
        }
        cursor = cursor.max(range.end);
    }
    (cursor < total).then_some(ByteRange {
        start: cursor,
        end: total.min(cursor.saturating_add(size)),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disjoint_dynamic_gaps_and_out_of_order_completion() {
        let mut completed = vec![];
        let a = next(&completed, &[], 100, 20).unwrap();
        let b = next(&completed, &[a], 100, 20).unwrap();
        complete(&mut completed, b);
        assert_eq!(next(&completed, &[a], 100, 20).unwrap().start, 40);
        complete(&mut completed, a);
        assert_eq!(completed, vec![ByteRange { start: 0, end: 40 }]);
        assert!(validate(&[a, a], 100).is_err());
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    #[test]
    fn arbitrary_completion_order_never_overlaps_or_loses_bytes() {
        for size in [1, 7, 1024, 32768] {
            let total = 12345;
            let mut completed = vec![];
            let mut active = vec![];
            let mut seed = 1u64;
            loop {
                while active.len() < 32 {
                    match next(&completed, &active, total, size) {
                        Some(range) => active.push(range),
                        None => break,
                    }
                }
                let mut occupied = completed.clone();
                occupied.extend(&active);
                occupied.sort_by_key(|r| r.start);
                validate(&occupied, total).unwrap();
                if active.is_empty() {
                    break;
                }
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let index = seed as usize % active.len();
                let range = active.remove(index);
                complete(&mut completed, range);
            }
            assert_eq!(
                completed,
                vec![ByteRange {
                    start: 0,
                    end: total
                }]
            );
        }
    }
}
