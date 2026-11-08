//! Locale-independent canonical caseless search keys, pinned to Unicode 17.
//!
//! NFD -> full default C+F case folding -> NFC. Display text and stable IDs must
//! remain unchanged; callers use this only for comparisons and substring search.
//! Immutable generated tables keep behavior independent of OS and Rust Unicode
//! versions. Work and scratch memory scale linearly with the caller-bounded input.

#[path = "unicode_data.rs"]
mod data;

/// Compare canonically equivalent text without case, retaining accent distinctions.
/// This is not locale-specific Turkic folding, compatibility normalization or an ID.
pub fn caseless_key(value: &str) -> String {
    if value.is_ascii() {
        return value.to_ascii_lowercase();
    }
    let mut decomposed = Vec::with_capacity(value.len());
    for point in value.chars() {
        decompose(point as u32, &mut decomposed);
    }
    reorder(&mut decomposed);

    // Fold decomposed text before normalizing again: U+0345 becomes a starter,
    // and full mappings can expose new compositions across an expansion.
    let mut folded = Vec::with_capacity(decomposed.len());
    for point in decomposed {
        if let Ok(index) = data::CASE_FOLD.binary_search_by_key(&point, |row| row.0) {
            let (_, first, second, third) = data::CASE_FOLD[index];
            decompose(first, &mut folded);
            if second != 0 {
                decompose(second, &mut folded);
            }
            if third != 0 {
                decompose(third, &mut folded);
            }
        } else {
            folded.push(point);
        }
    }
    reorder(&mut folded);
    compose(&folded)
}

fn decompose(point: u32, result: &mut Vec<u32>) {
    if (0xAC00..=0xD7A3).contains(&point) {
        let offset = point - 0xAC00;
        result.push(0x1100 + offset / 588);
        result.push(0x1161 + offset % 588 / 28);
        if offset % 28 != 0 {
            result.push(0x11A7 + offset % 28);
        }
    } else if let Ok(index) = data::CANONICAL.binary_search_by_key(&point, |row| row.0) {
        let (_, first, second) = data::CANONICAL[index];
        // The pinned generator proves acyclicity and maximum recursion depth 4.
        decompose(first, result);
        if second != 0 {
            decompose(second, result);
        }
    } else {
        result.push(point);
    }
}

fn combining_class(point: u32) -> u8 {
    use std::cmp::Ordering;
    data::COMBINING
        .binary_search_by(|&(start, end, _)| {
            if end < point {
                Ordering::Less
            } else if start > point {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .map_or(0, |index| data::COMBINING[index].2)
}

fn reorder(points: &mut [u32]) {
    let mut start = 0;
    let mut previous = 0;
    let mut unsorted = false;
    for end in 0..=points.len() {
        let order = points.get(end).map_or(0, |&point| combining_class(point));
        if order == 0 {
            if unsorted {
                reorder_run(&mut points[start..end]);
            }
            start = end + 1;
            unsorted = false;
        } else if order < previous {
            unsorted = true;
        }
        previous = order;
    }
}

/// Stable counting sort, avoiding quadratic insertion on long combining runs.
fn reorder_run(points: &mut [u32]) {
    let mut positions = [0_usize; 256];
    for &point in points.iter() {
        positions[usize::from(combining_class(point))] += 1;
    }
    let mut next = 0;
    for count in &mut positions {
        let size = *count;
        *count = next;
        next += size;
    }
    let mut ordered = vec![0; points.len()];
    for &point in points.iter() {
        let slot = &mut positions[usize::from(combining_class(point))];
        ordered[*slot] = point;
        *slot += 1;
    }
    points.copy_from_slice(&ordered);
}

fn compose_pair(first: u32, second: u32) -> Option<u32> {
    if (0x1100..0x1113).contains(&first) && (0x1161..0x1176).contains(&second) {
        return Some(0xAC00 + ((first - 0x1100) * 21 + second - 0x1161) * 28);
    }
    if (0xAC00..=0xD7A3).contains(&first)
        && (first - 0xAC00) % 28 == 0
        && (0x11A8..0x11C3).contains(&second)
    {
        return Some(first + second - 0x11A7);
    }
    data::COMPOSITION
        .binary_search_by_key(&(first, second), |row| (row.0, row.1))
        .ok()
        .map(|index| data::COMPOSITION[index].2)
}

fn compose(points: &[u32]) -> String {
    let mut composed = Vec::with_capacity(points.len());
    let mut starter = None;
    let mut previous_class = 0;
    for &point in points {
        let order = combining_class(point);
        if let Some(index) = starter {
            if previous_class == 0 || previous_class < order {
                if let Some(combined) = compose_pair(composed[index], point) {
                    composed[index] = combined;
                    continue;
                }
            }
        }
        if order == 0 {
            starter = Some(composed.len());
        }
        composed.push(point);
        previous_class = order;
    }
    composed
        .into_iter()
        .map(|point| char::from_u32(point).expect("validated Unicode scalar table"))
        .collect()
}

#[cfg(test)]
#[path = "unicode_tests.rs"]
mod tests;
