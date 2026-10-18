//! Which results of a ranked search are matches, and which are only the nearest of what there is.
//!
//! A ranked list always has an end only because we stop it; a nearest-neighbour search has no idea that nothing in the
//! library is a giraffe. Two tiers are cut from the best-first list of cosine similarities:
//!
//! - **strong**: what the search shows. A photo is strong when it clears every rule the model has:
//!   1. an *absolute* floor, for models with a sigmoid head (SigLIP): the model's own `sigmoid(scale * cos + bias)` is at
//!      least [`Rules::strong_probability`]. This is what lets a query nothing matches (a giraffe, in a library without
//!      one) come back empty;
//!   2. a *relative* rule: within [`Rules::strong_delta`] of the best photo's cosine, so a query with one great match does
//!      not trail off into the library;
//!   3. a *gap* rule: the list stops before the largest drop between neighbours, if that drop is at least
//!      [`Rules::gap`] (the tail of a ranked list is smooth; a cliff is a boundary). The first [`MIN_BEFORE_GAP`] photos
//!      are never cut by it.
//! - **weak**: the rest, down to a looser floor ([`Rules::weak_probability`] and [`Rules::weak_delta`]), shown only when the
//!   user asks ("Show weaker matches").
//!
//! CLIP has no absolute rule: its softmax-trained cosine similarities mean different things for different queries (a
//! good "dog" match scores about 0.30, a good "sunset over the sea" about 0.27), so no single number separates matches
//! from non-matches; only the relative and gap rules apply, and a query nothing matches still returns its nearest photos.
//!
//! The numbers were tuned on the 209-photo evaluation set (`tests/bakeoff.rs`, results in `docs/build.md`).
use crate::Model;

/// The gap rule never cuts before this many results.
pub const MIN_BEFORE_GAP: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    /// Strong: minimum `sigmoid(scale * cos + bias)`. `None` for models without a sigmoid head.
    pub strong_probability: Option<f32>,
    /// Strong: at most this far below the best cosine.
    pub strong_delta: f32,
    /// Strong: stop before a drop between neighbouring cosines of at least this.
    pub gap: f32,
    /// Weak: minimum probability (as `strong_probability`).
    pub weak_probability: Option<f32>,
    /// Weak: at most this far below the best cosine.
    pub weak_delta: f32,
}

impl Model {
    pub fn cutoff_rules(self) -> Rules {
        match self {
            Model::ClipB32 => Rules {
                strong_probability: None,
                strong_delta: 0.03,
                gap: 0.015,
                weak_probability: None,
                weak_delta: 0.04,
            },
            Model::Siglip2B16 => Rules {
                strong_probability: Some(0.003),
                strong_delta: 0.06,
                gap: 0.02,
                weak_probability: Some(0.001),
                weak_delta: 0.12,
            },
        }
    }
}

/// The rules for "More like this" (photo to photo), in cosine terms: there is no sigmoid head to read an absolute probability
/// from, so the floors are cosines of their own. See [`Model::image_cutoff_rules`] for the numbers and where they came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageRules {
    /// Strong: minimum cosine to the source photo.
    pub strong_min: f32,
    /// Strong: at most this far below the best cosine.
    pub strong_delta: f32,
    /// Strong: stop before a drop between neighbouring cosines of at least this.
    pub gap: f32,
    /// Weak: minimum cosine.
    pub weak_min: f32,
    /// Weak: at most this far below the best cosine.
    pub weak_delta: f32,
}

impl Model {
    /// Where "More like this" cuts its list (photo to photo; `docs/library.md`, "More like this").
    ///
    /// Derived on the 209-photo evaluation set with `similar_bakeoff` in `tests/bakeoff.rs` (`just bakeoff`): every photo is
    /// the source once, the other 208 are ranked by cosine, a result is relevant when it is in the source's category. The
    /// photo-to-photo cosines are not the text-to-photo ones: they run from about 0.4 to 0.97 (the 99th percentile of
    /// pairs from different categories is 0.68 for CLIP and 0.70 for SigLIP 2), so the text rules (`cutoff_rules`: SigLIP 2's
    /// probability floor reads image-text cosines of about 0.1 to 0.2) cannot be reused, and there is no per-query
    /// probability to lean on. Unlike for a text query, an absolute cosine means about the same for any source photo, so
    /// both models get an absolute floor:
    /// - `strong_min`: a precision of 0.92 (wrong one time in twelve) at the smallest floor that gets there with a relative
    ///   distance of 0.10: CLIP 0.74 (precision 0.919, recall 0.224, 2.2 photos per source), SigLIP 2 0.76 (0.922, 0.196, 1.9);
    /// - `strong_delta` 0.10 below the best neighbour, so a source with a near-identical photo (0.95) does not list the loosely
    ///   related ones (0.76) behind it as equals; `gap` 0.05, the same guard against a cliff as for text (it changed
    ///   precision and recall by less than 0.01 on this set);
    /// - `weak_min`: the 99th percentile of other-category pairs (CLIP 0.68, SigLIP 2 0.70), within 0.20 of the best; strong
    ///   plus weaker has precision 0.67 and recall 0.40 to 0.41 (5.3 to 5.5 photos per source), against 0.52 / 0.58 for a plain top 10.
    pub fn image_cutoff_rules(self) -> ImageRules {
        match self {
            Model::ClipB32 => ImageRules {
                strong_min: 0.74,
                strong_delta: 0.10,
                gap: 0.05,
                weak_min: 0.68,
                weak_delta: 0.20,
            },
            Model::Siglip2B16 => ImageRules {
                strong_min: 0.76,
                strong_delta: 0.10,
                gap: 0.05,
                weak_min: 0.70,
                weak_delta: 0.20,
            },
        }
    }
}

/// How many of the best-first results are strong and how many are strong or weak (`strong <= weak <= len`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tiers {
    pub strong: usize,
    pub weak: usize,
}

/// Apply the model's rules to `cosines`, sorted best first.
pub fn tiers(model: Model, cosines: &[f32]) -> Tiers {
    let rules = model.cutoff_rules();
    let floor = |p: Option<f32>| match (p, model.calibration()) {
        (Some(p), Some(c)) => c.cosine_at(p),
        _ => f32::NEG_INFINITY,
    };
    cut(
        cosines,
        Cut {
            strong_min: floor(rules.strong_probability),
            strong_delta: rules.strong_delta,
            gap: rules.gap,
            weak_min: floor(rules.weak_probability),
            weak_delta: rules.weak_delta,
        },
    )
}

/// Strong and weaker tiers of a photo-to-photo list (`cosines` best first, the source photo itself not among them).
pub fn image_tiers(model: Model, cosines: &[f32]) -> Tiers {
    image_tiers_with(model.image_cutoff_rules(), cosines)
}

/// [`image_tiers`] with the rules given (the calibration in `tests/bakeoff.rs` sweeps them).
pub fn image_tiers_with(r: ImageRules, cosines: &[f32]) -> Tiers {
    cut(
        cosines,
        Cut {
            strong_min: r.strong_min,
            strong_delta: r.strong_delta,
            gap: r.gap,
            weak_min: r.weak_min,
            weak_delta: r.weak_delta,
        },
    )
}

/// The rules of a list in cosine terms (an absolute floor, a distance below the best, a cliff size), whatever they were
/// derived from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cut {
    strong_min: f32,
    strong_delta: f32,
    gap: f32,
    weak_min: f32,
    weak_delta: f32,
}

fn cut(cosines: &[f32], r: Cut) -> Tiers {
    let Some(&best) = cosines.first() else {
        return Tiers { strong: 0, weak: 0 };
    };
    let count_above = |min: f32| cosines.iter().take_while(|&&c| c >= min).count();
    let strong_min = r.strong_min.max(best - r.strong_delta);
    let mut strong = count_above(strong_min);
    // The largest drop between neighbours, if it is a cliff.
    if strong > MIN_BEFORE_GAP {
        let (mut at, mut size) = (0, 0.0f32);
        for i in MIN_BEFORE_GAP..strong {
            let drop = cosines[i - 1] - cosines[i];
            if drop > size {
                (at, size) = (i, drop);
            }
        }
        if size >= r.gap {
            strong = at;
        }
    }
    let weak_min = r.weak_min.max(best - r.weak_delta);
    let weak = count_above(weak_min).max(strong);
    Tiers { strong, weak }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cosines for a SigLIP list given as probabilities (best first).
    fn siglip(probs: &[f32]) -> Vec<f32> {
        let c = Model::Siglip2B16.calibration().unwrap();
        probs.iter().map(|&p| c.cosine_at(p)).collect()
    }

    #[test]
    fn an_empty_list_has_no_tiers() {
        assert_eq!(tiers(Model::Siglip2B16, &[]), Tiers { strong: 0, weak: 0 });
    }

    #[test]
    fn siglip_returns_nothing_strong_when_nothing_clears_the_floor() {
        // A query the library has no answer to: the best photo is 0.04 % likely.
        let t = tiers(Model::Siglip2B16, &siglip(&[0.0004, 0.0003, 0.0002, 0.0001]));
        assert_eq!(t, Tiers { strong: 0, weak: 0 });
        // Probability 0.25 % is weak, not strong.
        let t = tiers(Model::Siglip2B16, &siglip(&[0.0025, 0.002, 0.0002]));
        assert_eq!(t.strong, 0);
        assert_eq!(t.weak, 2);
    }

    #[test]
    fn siglip_strong_stops_where_the_probability_floor_does() {
        let t = tiers(
            Model::Siglip2B16,
            &siglip(&[0.05, 0.04, 0.03, 0.02, 0.01, 0.0055, 0.0035, 0.0025, 0.0004]),
        );
        assert_eq!(t.strong, 7);
        assert_eq!(t.weak, 8);
    }

    #[test]
    fn the_relative_rule_trims_a_list_far_below_its_best() {
        // A caption-like query with an excellent top match (p = 0.6, cos 0.14): only photos within 0.06 stay strong even
        // though every one of them clears the absolute floor.
        let cos = [0.30, 0.29, 0.28, 0.27, 0.26, 0.23, 0.2, 0.18, 0.15, 0.1];
        let t = tiers(Model::Siglip2B16, &cos);
        assert_eq!(t.strong, 5, "{t:?}");
        assert!(t.weak >= 8, "{t:?}");
    }

    #[test]
    fn the_gap_rule_stops_at_the_largest_cliff_but_never_before_the_first_few() {
        let model = Model::ClipB32;
        // Smooth, then a 0.02 cliff after the sixth result, then smooth again (all within 0.03 of the best).
        let cos = [0.30, 0.299, 0.298, 0.297, 0.296, 0.295, 0.275, 0.274, 0.273];
        assert_eq!(tiers(model, &cos).strong, 6);
        // A cliff right at the top (after the first) is not a boundary: the best photos always stay.
        let cos = [0.30, 0.28, 0.279, 0.278, 0.277];
        assert_eq!(tiers(model, &cos).strong, 5);
        // The largest of several cliffs wins.
        let cos = [0.30, 0.299, 0.298, 0.282, 0.281, 0.26, 0.259];
        assert_eq!(tiers(model, &cos).strong, 3);
        // Below the gap size nothing is cut.
        let cos = [0.30, 0.299, 0.298, 0.297, 0.285, 0.284];
        assert_eq!(tiers(model, &cos).strong, 6);
    }

    #[test]
    fn clip_has_no_absolute_floor() {
        // A query nothing matches: CLIP still returns its nearest photos as strong (documented limitation).
        let t = tiers(Model::ClipB32, &[0.12, 0.119, 0.118, 0.1, 0.05]);
        assert_eq!(t.strong, 3);
        assert_eq!(t.weak, 4, "within 0.04 of the best");
    }

    #[test]
    fn weak_is_never_smaller_than_strong() {
        for model in Model::ALL {
            for cos in [vec![0.5; 10], vec![0.3, 0.1, 0.0, -0.1], vec![0.2]] {
                let t = tiers(model, &cos);
                assert!(t.strong <= t.weak && t.weak <= cos.len(), "{model:?} {t:?}");
            }
        }
    }

    #[test]
    fn image_lists_need_a_close_photo_to_have_a_strong_tier() {
        for model in Model::ALL {
            let r = model.image_cutoff_rules();
            // Nothing near the source: no strong match, a weaker tier for what is moderately close.
            let t = image_tiers(
                model,
                &[
                    r.strong_min - 0.01,
                    r.strong_min - 0.02,
                    r.weak_min + 0.01,
                    r.weak_min - 0.05,
                ],
            );
            assert_eq!(t, Tiers { strong: 0, weak: 3 }, "{model:?}");
            // An empty library has nothing to list.
            assert_eq!(image_tiers(model, &[]), Tiers { strong: 0, weak: 0 });
        }
    }

    #[test]
    fn image_strong_is_relative_to_the_best_neighbour() {
        let model = Model::Siglip2B16;
        let r = model.image_cutoff_rules();
        // A near-duplicate at 0.97: photos within 0.10 stay; the one at 0.80 is over the floor but not close to the best, so it
        // is weaker, and so is nothing under 0.77 (within 0.20).
        let cos = [0.97, 0.95, 0.90, 0.80, 0.72, 0.6];
        let t = image_tiers(model, &cos);
        assert_eq!(t, Tiers { strong: 3, weak: 4 });
        // Without a near-duplicate the floor decides.
        let t = image_tiers(model, &[r.strong_min + 0.01, r.strong_min, r.strong_min - 0.001]);
        assert_eq!(t.strong, 2);
    }
}
