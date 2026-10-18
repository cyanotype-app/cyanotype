//! Bursts in a ranked list: the photos of one burst (`burst_id`, `cyanotype_core::bursts`) that are in the list collapse into
//! one result. The list is the whole ranked list (at most 300 photos), so this runs before the tiers are cut and the page is
//! taken, and everything after it counts groups, not photos.
//!
//! - a group is ranked by its best photo (the first of its photos in the list), so the list's order of groups is the order
//!   in which their first photo appears;
//! - its **cover**, the photo shown, is the best-ranked favorite of the group if it has one, else the best-ranked photo;
//! - a photo without a burst is a group of one.
use std::collections::HashMap;

use uuid::Uuid;

use super::search::SearchRow;

/// One result: row positions in the ranked list.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Group {
    /// The photo shown for the group.
    pub(super) cover: usize,
    /// The group's best-ranked photo (what it ranks by, and what its tier is cut from).
    pub(super) cover_best: usize,
    /// The other photos, best first.
    pub(super) others: Vec<usize>,
}

/// `(burst_id, favorite)` per row, best first, to groups, best first.
fn collapse_keys(keys: &[(Option<Uuid>, bool)]) -> Vec<Group> {
    let mut members: Vec<Vec<usize>> = Vec::new();
    let mut by_burst: HashMap<Uuid, usize> = HashMap::new();
    for (i, (burst, _)) in keys.iter().enumerate() {
        match burst {
            Some(b) => match by_burst.get(b) {
                Some(&g) => members[g].push(i),
                None => {
                    by_burst.insert(*b, members.len());
                    members.push(vec![i]);
                }
            },
            None => members.push(vec![i]),
        }
    }
    members
        .into_iter()
        .map(|m| {
            let cover = m.iter().copied().find(|&i| keys[i].1).unwrap_or(m[0]);
            Group {
                cover,
                cover_best: m[0],
                others: m.iter().copied().filter(|&i| i != cover).collect(),
            }
        })
        .collect()
}

pub(super) fn collapse(rows: &[SearchRow]) -> Vec<Group> {
    let keys: Vec<(Option<Uuid>, bool)> = rows.iter().map(|r| (r.burst_id, r.row.summary.favorite)).collect();
    collapse_keys(&keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Option<Uuid> {
        Some(Uuid::from_u128(n))
    }

    #[test]
    fn photos_on_their_own_stay_in_order_and_a_burst_takes_the_place_of_its_best_photo() {
        // rank: a (own), b1 (burst 1), c (own), b2 (burst 1), b3 (burst 1), d (own)
        let keys = [
            (None, false),
            (id(1), false),
            (None, false),
            (id(1), false),
            (id(1), false),
            (None, false),
        ];
        let g = collapse_keys(&keys);
        assert_eq!(g.len(), 4);
        assert_eq!((g[0].cover, g[0].others.clone()), (0, vec![]));
        assert_eq!((g[1].cover, g[1].cover_best, g[1].others.clone()), (1, 1, vec![3, 4]));
        assert_eq!((g[2].cover, g[3].cover), (2, 5));
    }

    #[test]
    fn a_favorite_is_the_cover_but_the_group_ranks_by_its_best_photo() {
        let keys = [(id(7), false), (id(7), true), (id(7), true), (None, false)];
        let g = collapse_keys(&keys);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].cover, 1, "the best-ranked favorite");
        assert_eq!(g[0].cover_best, 0, "ranked and tiered by its best photo");
        assert_eq!(g[0].others, vec![0, 2]);
    }

    #[test]
    fn two_bursts_interleave_and_an_empty_list_has_no_groups() {
        let keys = [(id(1), false), (id(2), false), (id(1), false), (id(2), false)];
        let g = collapse_keys(&keys);
        assert_eq!(g.iter().map(|g| g.cover).collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(g[0].others, vec![2]);
        assert_eq!(g[1].others, vec![3]);
        assert!(collapse_keys(&[]).is_empty());
    }
}
