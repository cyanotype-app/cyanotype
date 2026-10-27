//! The arithmetic of People (docs/faces.md, "People"), with no database: the thresholds, carrying a photo's assignments over to the
//! faces of a new scan of it, choosing the person a new face joins, and grouping unassigned faces. Everything works on unit-length
//! fingerprints, so a cosine is a dot product. The thresholds were measured on the evaluation set of `cyanotype-faces/tests/bakeoff.rs`
//! (`clustering`, ignored); the numbers are next to each constant and in docs/faces.md.
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// SFace's published threshold for "the same person" (cosine). Used where two faces are known to be of the same photo, so a
/// stranger is far less likely than in a library: carrying a decision over to the new scan of its photo (below).
pub const SAME_FACE: f32 = 0.363;
/// Two boxes of one photo are the same face when they overlap at least this much (a face without a fingerprint, or another model).
pub const SAME_BOX_IOU: f32 = 0.5;

/// A new face joins an existing person when its nearest face of that person is at least this similar. High on purpose: the figure
/// that matters is not the chance that two photos of strangers agree (0.1 % of pairs are above cosine 0.349) but the chance that ANY of
/// the faces of all the people already known looks like a stranger's, which grows with the library. Measured on the evaluation set
/// (docs/faces.md): no wrong assignment at 0.40 and up, 90.8 % right at 0.55 and 80.8 % at 0.60 (the rest stay unassigned and are
/// grouped later); extrapolating the strangers' tail, the chance that one of 10,000 known faces reaches 0.60 is about 1 %, 0.55 is 5 %,
/// 0.50 is 20 %. Splitting a person costs a merge; joining a stranger costs trust: 0.60.
pub const ASSIGN: f32 = 0.60;
/// ... and the second best person must be this much further away, or the face stays unassigned (two look-alikes).
pub const ASSIGN_MARGIN: f32 = 0.08;
/// Unassigned faces are grouped starting from pairs at least this similar (the edges of the graph). Single-link components of this
/// set are 100 % pure from 0.45 up, but chain strangers in a big library, so the edges are stricter than the join below ...
pub const LINK: f32 = 0.55;
/// ... and two groups join only while the cosine of their centres (the mean fingerprint of each) is at least this: single links alone
/// chain A to B to C through look-alikes, a centre does not. Measured: the centres of two halves of one person's faces are at least
/// 0.574 apart, those of two different people at most 0.450.
pub const MERGE: f32 = 0.50;
/// A new unnamed person needs at least this many faces, from at least [`MIN_PHOTOS`] photos; fewer wait until they grow.
pub const MIN_FACES: usize = 3;
pub const MIN_PHOTOS: usize = 2;
/// A group joins an existing person (instead of becoming a new one) when the cosine of their centres is at least this: between the
/// same-person minimum (0.574) and the different-people maximum (0.450) measured for two centres.
pub const JOIN: f32 = 0.55;
/// "Is this X?" lists the unassigned faces whose similarity to the centre of X is at least this, which is lower than [`ASSIGN`] because
/// the owner confirms each one. Measured: at 0.45 every suggestion on the set was right and 98.5 % of the right ones were made; at 0.35,
/// 91 % were right.
pub const SUGGEST: f32 = 0.45;
/// Nearest neighbours asked of the index per face when looking for edges, and for a person to join.
pub const NEIGHBOURS: i64 = 12;

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Cosine of two vectors that need not be unit length.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (na, nb) = (dot(a, a).sqrt(), dot(b, b).sqrt());
    dot(a, b) / (na * nb).max(f32::MIN_POSITIVE)
}

/// pgvector's text form (`[0.1,0.2]`) back to numbers; `None` when it is not one.
pub fn parse_literal(s: &str) -> Option<Vec<f32>> {
    let inner = s.trim().strip_prefix('[')?.strip_suffix(']')?;
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }
    inner.split(',').map(|x| x.trim().parse::<f32>().ok()).collect()
}

/// Intersection over union of two boxes (left, top, width, height).
pub fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let (ax2, ay2, bx2, by2) = (a[0] + a[2], a[1] + a[3], b[0] + b[2], b[1] + b[3]);
    let iw = (ax2.min(bx2) - a[0].max(b[0])).max(0.0);
    let ih = (ay2.min(by2) - a[1].max(b[1])).max(0.0);
    let inter = iw * ih;
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// What a scan knows of one face, for matching it to the same face of an earlier scan.
#[derive(Debug, Clone, Copy)]
pub struct Seen<'a> {
    pub bbox: [f32; 4],
    /// The fingerprint and the id of the model that made it: numbers of two models mean nothing next to each other.
    pub embedding: Option<(&'a str, &'a [f32])>,
}

/// Which earlier face is each new face of the same photo: the best fingerprint match above [`SAME_FACE`] first (the faces moved:
/// a rotate or a crop), then the boxes that overlap by [`SAME_BOX_IOU`] (a face without a fingerprint, or a new version of the
/// model that kept the picture). One to one: an earlier face answers at most one new face, the strongest pair first.
/// Fingerprints of another model are not compared, only boxes.
pub fn match_faces(old: &[Seen], new: &[Seen]) -> Vec<Option<usize>> {
    match_by(old, new, SAME_FACE, SAME_BOX_IOU)
}

/// [`match_faces`] with the two thresholds given: the cosine at least which two fingerprints of one model are the same thing, and the
/// overlap at least which two boxes are (named pets match their animals with their own, `crate::pets`).
pub fn match_by(old: &[Seen], new: &[Seen], same_embedding: f32, same_box: f32) -> Vec<Option<usize>> {
    let mut out: Vec<Option<usize>> = vec![None; new.len()];
    let mut taken = vec![false; old.len()];
    let mut pass = |score: &dyn Fn(&Seen, &Seen) -> Option<f32>| {
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        for (i, n) in new.iter().enumerate() {
            if out[i].is_some() {
                continue;
            }
            for (j, o) in old.iter().enumerate() {
                if !taken[j]
                    && let Some(s) = score(n, o)
                {
                    pairs.push((s, i, j));
                }
            }
        }
        pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        for (_, i, j) in pairs {
            if out[i].is_none() && !taken[j] {
                out[i] = Some(j);
                taken[j] = true;
            }
        }
    };
    pass(&|n, o| match (n.embedding, o.embedding) {
        (Some((ma, a)), Some((mb, b))) if ma == mb && a.len() == b.len() => {
            Some(dot(a, b)).filter(|c| *c >= same_embedding)
        }
        _ => None,
    });
    pass(&|n, o| Some(iou(&n.bbox, &o.bbox)).filter(|v| *v >= same_box));
    out
}

/// The person a new face joins, from its nearest fingerprints that already have a person: `(person, similarity)` pairs, any order.
/// The person with the single most similar face wins when that is at least [`ASSIGN`] and the next person is at least
/// [`ASSIGN_MARGIN`] behind; people the face was rejected from do not count at all.
pub fn choose_person<P: Copy + Eq + Hash>(
    neighbours: &[(P, f32)],
    rejected: &HashSet<P>,
    threshold: f32,
    margin: f32,
) -> Option<P> {
    let mut best: HashMap<P, f32> = HashMap::new();
    for (p, s) in neighbours {
        if rejected.contains(p) {
            continue;
        }
        let e = best.entry(*p).or_insert(f32::NEG_INFINITY);
        *e = e.max(*s);
    }
    let mut ranked: Vec<(P, f32)> = best.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let (top, s) = *ranked.first()?;
    if s < threshold {
        return None;
    }
    if ranked.get(1).is_some_and(|(_, second)| s - second < margin) {
        return None;
    }
    Some(top)
}

/// One face to group: its fingerprint and the photo it is in.
pub struct Node<'a> {
    pub embedding: &'a [f32],
    pub photo: u32,
}

/// Group faces: `edges` are `(similarity, a, b)` pairs of node indexes at least [`LINK`] apart (a nearest-neighbour graph is enough).
/// Strongest first, two groups join when no photo is in both (two faces of one photo are two people) and the cosine of their
/// centres is at least `merge`. A pair of look-alikes joins two strangers' chains with a single link; the centres of two large groups of different people
/// are far apart, so it does not. Returns the groups (node indexes, in order), singletons included.
pub fn group(nodes: &[Node], edges: &mut [(f32, u32, u32)], merge: f32) -> Vec<Vec<usize>> {
    struct G {
        sum: Vec<f32>,
        members: Vec<usize>,
        photos: HashSet<u32>,
    }
    let mut parent: Vec<usize> = (0..nodes.len()).collect();
    let mut groups: Vec<Option<G>> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            Some(G {
                sum: n.embedding.to_vec(),
                members: vec![i],
                photos: HashSet::from([n.photo]),
            })
        })
        .collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    edges.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    for &(_, a, b) in edges.iter() {
        let (ra, rb) = (find(&mut parent, a as usize), find(&mut parent, b as usize));
        if ra == rb {
            continue;
        }
        let (ga, gb) = (groups[ra].as_ref().unwrap(), groups[rb].as_ref().unwrap());
        if !ga.photos.is_disjoint(&gb.photos) || cosine(&ga.sum, &gb.sum) < merge {
            continue;
        }
        // The smaller joins the larger.
        let (keep, gone) = if ga.members.len() >= gb.members.len() {
            (ra, rb)
        } else {
            (rb, ra)
        };
        let g = groups[gone].take().unwrap();
        let k = groups[keep].as_mut().unwrap();
        k.sum.iter_mut().zip(&g.sum).for_each(|(x, y)| *x += y);
        k.members.extend(g.members);
        k.photos.extend(g.photos);
        parent[gone] = keep;
    }
    let mut out: Vec<Vec<usize>> = groups
        .into_iter()
        .flatten()
        .map(|mut g| {
            g.members.sort_unstable();
            g.members
        })
        .collect();
    out.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    out
}

/// Is a group big enough to be a person of its own?
pub fn big_enough(faces: usize, photos: usize) -> bool {
    faces >= MIN_FACES && photos >= MIN_PHOTOS
}

/// The mean of unit vectors, as a unit vector (`None` for no vectors or a zero sum).
pub fn centre<'a>(vs: impl IntoIterator<Item = &'a [f32]>) -> Option<Vec<f32>> {
    let mut sum: Vec<f32> = Vec::new();
    for v in vs {
        if sum.is_empty() {
            sum = v.to_vec();
        } else {
            sum.iter_mut().zip(v).for_each(|(x, y)| *x += y);
        }
    }
    let n = dot(&sum, &sum).sqrt();
    (n > 0.0).then(|| sum.iter().map(|x| x / n).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(v: &[f32]) -> Vec<f32> {
        let n = dot(v, v).sqrt();
        v.iter().map(|x| x / n).collect()
    }

    #[test]
    fn a_vector_survives_its_text_form_and_nonsense_is_refused() {
        assert_eq!(parse_literal("[0.5,-1,2e-1]"), Some(vec![0.5, -1.0, 0.2]));
        assert_eq!(parse_literal(" [ 1 , 2 ] "), Some(vec![1.0, 2.0]));
        assert_eq!(parse_literal("[]"), Some(vec![]));
        for bad in ["", "1,2", "[1,x]", "[1,2"] {
            assert_eq!(parse_literal(bad), None, "{bad:?}");
        }
        let v = [0.25f32, -0.125, 3.0];
        assert_eq!(parse_literal(&crate::embeddings::pg_literal(&v)), Some(v.to_vec()));
    }

    #[test]
    fn boxes_overlap_by_union() {
        let a = [0.1, 0.1, 0.2, 0.2];
        assert!((iou(&a, &a) - 1.0).abs() < 1e-6);
        assert_eq!(iou(&a, &[0.5, 0.5, 0.1, 0.1]), 0.0);
        // Half of each: 0.02 / (0.04 + 0.04 - 0.02).
        assert!((iou(&a, &[0.2, 0.1, 0.2, 0.2]) - 1.0 / 3.0).abs() < 1e-5);
    }

    fn seen<'a>(bbox: [f32; 4], e: Option<&'a [f32]>) -> Seen<'a> {
        Seen {
            bbox,
            embedding: e.map(|e| ("m", e)),
        }
    }

    #[test]
    fn faces_of_a_new_scan_find_their_old_selves_by_fingerprint_even_when_they_moved() {
        let (e1, e2) = (unit(&[1.0, 0.0, 0.1]), unit(&[0.0, 1.0, 0.1]));
        // Rotated: both faces are somewhere else, and listed in the other order.
        let old = [
            seen([0.1, 0.1, 0.2, 0.2], Some(&e1)),
            seen([0.6, 0.1, 0.2, 0.2], Some(&e2)),
        ];
        let new = [
            seen([0.1, 0.6, 0.2, 0.2], Some(&e2)),
            seen([0.7, 0.7, 0.2, 0.2], Some(&e1)),
        ];
        assert_eq!(match_faces(&old, &new), vec![Some(1), Some(0)]);
    }

    #[test]
    fn a_face_without_a_fingerprint_is_matched_by_its_box_and_each_old_face_answers_one_new_face() {
        let e = unit(&[1.0, 0.0]);
        let old = [seen([0.1, 0.1, 0.2, 0.2], None), seen([0.5, 0.5, 0.2, 0.2], Some(&e))];
        // The first new face overlaps the first old one; the second overlaps it too but is the weaker pair; the third
        // has a fingerprint unlike anything.
        let other = unit(&[0.0, 1.0]);
        let new = [
            seen([0.1, 0.1, 0.2, 0.2], None),
            seen([0.12, 0.1, 0.2, 0.2], None),
            seen([0.9, 0.9, 0.05, 0.05], Some(&other)),
        ];
        assert_eq!(match_faces(&old, &new), vec![Some(0), None, None]);
    }

    #[test]
    fn fingerprints_of_another_model_are_not_compared_only_boxes_are() {
        let e = unit(&[1.0, 0.0]);
        let old = [seen([0.1, 0.1, 0.2, 0.2], Some(&e))];
        // The same numbers at another place: the same model says yes; another model's numbers mean nothing, only the box counts.
        let moved = [seen([0.6, 0.6, 0.2, 0.2], Some(&e))];
        assert_eq!(match_faces(&old, &moved), vec![Some(0)]);
        let mut other = moved;
        other[0].embedding = Some(("another", &e));
        assert_eq!(match_faces(&old, &other), vec![None]);
        let o2 = unit(&[0.0, 1.0]);
        let mut same_place = [seen([0.1, 0.1, 0.2, 0.2], Some(&o2))];
        same_place[0].embedding = Some(("another", &o2));
        assert_eq!(match_faces(&old, &same_place), vec![Some(0)]);
    }

    #[test]
    fn the_nearest_person_wins_only_when_clearly_near_and_clearly_ahead() {
        let none = HashSet::new();
        assert_eq!(
            choose_person(&[(1, 0.7), (1, 0.3), (2, 0.4)], &none, 0.55, 0.08),
            Some(1)
        );
        // Too far.
        assert_eq!(choose_person(&[(1, 0.5)], &none, 0.55, 0.08), None);
        // Two people alike: neither.
        assert_eq!(choose_person(&[(1, 0.7), (2, 0.66)], &none, 0.55, 0.08), None);
        // The same person's second face does not count as a rival.
        assert_eq!(choose_person(&[(1, 0.7), (1, 0.69)], &none, 0.55, 0.08), Some(1));
        // A person it was rejected from is not a candidate, and takes no part in the margin either.
        let r = HashSet::from([1]);
        assert_eq!(choose_person(&[(1, 0.9), (2, 0.6)], &r, 0.55, 0.08), Some(2));
        assert_eq!(choose_person(&[(1, 0.9)], &r, 0.55, 0.08), None);
        assert_eq!(choose_person::<i32>(&[], &none, 0.55, 0.08), None);
    }

    fn axis(i: usize, wobble: f32) -> Vec<f32> {
        let mut v = vec![0.0; 4];
        v[i] = 1.0;
        v[(i + 1) % 4] = wobble;
        unit(&v)
    }

    #[test]
    fn faces_of_one_photo_are_never_one_person_and_a_chain_of_look_alikes_does_not_join_strangers() {
        // Three clusters on three axes; a bridge face halfway between 0 and 1 links their chains.
        let mut vecs: Vec<Vec<f32>> = Vec::new();
        let mut photos: Vec<u32> = Vec::new();
        for (axis_i, base) in [(0usize, 0u32), (1, 100)] {
            for k in 0..4 {
                vecs.push(axis(axis_i, 0.02 * k as f32));
                photos.push(base + k);
            }
        }
        vecs.push(unit(&[1.0, 1.0, 0.0, 0.0]));
        photos.push(200);
        // Two faces of the same photo that are nearly the same vector.
        vecs.push(axis(0, 0.0));
        photos.push(0);
        let nodes: Vec<Node> = vecs
            .iter()
            .zip(&photos)
            .map(|(v, p)| Node {
                embedding: v,
                photo: *p,
            })
            .collect();
        let mut edges = Vec::new();
        for i in 0..nodes.len() {
            for j in i + 1..nodes.len() {
                let s = dot(nodes[i].embedding, nodes[j].embedding);
                if s >= LINK {
                    edges.push((s, i as u32, j as u32));
                }
            }
        }
        let g = group(&nodes, &mut edges, MERGE);
        // The two axes stay two groups of four; the bridge (0.7 to both) joins one of them or stays alone, never both; the
        // twin of face 0 is in the same photo as it, so it is not in face 0's group.
        let of = |i: usize| g.iter().position(|c| c.contains(&i)).unwrap();
        assert_ne!(of(0), of(4), "two people were joined");
        assert!((0..4).all(|i| of(i) == of(0)) && (4..8).all(|i| of(i) == of(4)));
        assert_ne!(of(0), of(9), "two faces of one photo were joined");
        assert!(g.iter().all(|c| {
            let ph: HashSet<u32> = c.iter().map(|i| photos[*i]).collect();
            ph.len() == c.len()
        }));
    }

    #[test]
    fn a_group_needs_enough_faces_from_enough_photos_and_a_centre_is_a_unit_mean() {
        assert!(big_enough(3, 2) && big_enough(10, 10));
        assert!(!big_enough(2, 2) && !big_enough(3, 1));
        let c = centre([&[1.0f32, 0.0][..], &[0.0, 1.0][..]]).unwrap();
        assert!((dot(&c, &c) - 1.0).abs() < 1e-6 && (c[0] - c[1]).abs() < 1e-6);
        assert!(centre([&[1.0f32, 0.0][..], &[-1.0, 0.0][..]]).is_none());
        assert!(centre(std::iter::empty::<&[f32]>()).is_none());
    }

    #[test]
    fn the_thresholds_are_ordered_the_way_the_rules_assume() {
        // Joining a person is harder than linking, which is harder than suggesting; the centre rules are not stricter than a link.
        const { assert!(SUGGEST < MERGE && MERGE <= LINK && LINK <= JOIN && JOIN <= ASSIGN) };
        const { assert!(ASSIGN > SAME_FACE && SUGGEST > 0.0) };
    }
}
