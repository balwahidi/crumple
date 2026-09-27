//! Per-codec quality-target search (ARCHITECTURE §4.1).

use std::collections::BTreeMap;

/// Search configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchConfig {
    pub target: f64,
    pub q_min: u8,
    pub q_max: u8,
    pub initial_q: u8,
    pub max_evals: u8,
    pub step: u8,
    pub score_slack: f64,
}

impl SearchConfig {
    /// Defaults: q_min 1, q_max 100, max_evals 6, step 8, slack 0.5.
    pub fn new(target: f64, initial_q: u8) -> Self {
        SearchConfig {
            target,
            q_min: 1,
            q_max: 100,
            initial_q,
            max_evals: 6,
            step: 8,
            score_slack: 0.5,
        }
    }
}

/// Result of a search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    Found {
        q: u8,
        bytes: u64,
        score: f64,
        evals: u8,
    },
    Unreachable {
        best_q: u8,
        best_score: f64,
        evals: u8,
    },
    Pruned {
        evals: u8,
    },
}

/// The next action the caller must perform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Encode(u8),
    Score(u8),
    Finished(Outcome),
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    bytes: u64,
    score: Option<f64>,
    too_big: bool,
}

/// Step-by-step planner; the caller performs encodes and scores.
#[derive(Debug, Clone)]
pub struct Planner {
    cfg: SearchConfig,
    size_bound: Option<u64>,
    cache: BTreeMap<u8, Entry>,
    lo: Option<(u8, f64)>,
    hi: Option<(u8, f64)>,
    cap: i32,
    evals: u8,
    last_q: Option<u8>,
    pending: Option<Step>,
    pending_interp: bool,
    /// Sides (true = passed) of the picks made in the "both seen" branch, in order.
    interp_sides: Vec<bool>,
    finished: Option<Outcome>,
}

impl Planner {
    pub fn new(cfg: SearchConfig, size_bound: Option<u64>) -> Planner {
        Planner {
            cfg,
            size_bound,
            cache: BTreeMap::new(),
            lo: None,
            hi: None,
            cap: cfg.q_max as i32,
            evals: 0,
            last_q: None,
            pending: None,
            pending_interp: false,
            interp_sides: Vec::new(),
            finished: None,
        }
    }

    /// Idempotent until the matching `on_*` call.
    #[allow(clippy::should_implement_trait)] // name fixed by the ARCHITECTURE §9 T3 API
    pub fn next(&mut self) -> Step {
        if let Some(o) = self.finished {
            return Step::Finished(o);
        }
        if let Some(s) = self.pending {
            return s;
        }
        let step = if self.evals == 0 {
            let q0 = self
                .clamp(self.cfg.initial_q as i32)
                .max(self.cfg.q_min as i32);
            Step::Encode(q0 as u8)
        } else if let Some(o) = self.stop_check() {
            Step::Finished(o)
        } else {
            match self.pick() {
                Some(q) => Step::Encode(q),
                None => Step::Finished(self.fallback()),
            }
        };
        match step {
            Step::Finished(o) => self.finished = Some(o),
            s => self.pending = Some(s),
        }
        step
    }

    /// Panics if `q` is not the pending Encode.
    pub fn on_encoded(&mut self, q: u8, bytes: u64) {
        assert_eq!(
            self.pending,
            Some(Step::Encode(q)),
            "on_encoded does not match the pending step"
        );
        self.evals += 1;
        self.last_q = Some(q);
        let too_big = self.size_bound.is_some_and(|b| bytes >= b);
        self.cache.insert(
            q,
            Entry {
                bytes,
                score: None,
                too_big,
            },
        );
        if too_big {
            self.cap = self.cap.min(q as i32 - 1);
            self.pending = None;
            self.pending_interp = false;
        } else {
            self.pending = Some(Step::Score(q));
        }
    }

    /// Panics if `q` is not the pending Score.
    pub fn on_scored(&mut self, q: u8, score: f64) {
        assert_eq!(
            self.pending,
            Some(Step::Score(q)),
            "on_scored does not match the pending step"
        );
        self.pending = None;
        if let Some(e) = self.cache.get_mut(&q) {
            e.score = Some(score);
        }
        let passed = score >= self.cfg.target;
        if passed {
            if self.hi.is_none_or(|(h, _)| q < h) {
                self.hi = Some((q, score));
            }
        } else if self.lo.is_none_or(|(l, _)| q > l) {
            self.lo = Some((q, score));
        }
        if self.pending_interp {
            self.interp_sides.push(passed);
            self.pending_interp = false;
        }
    }

    /// Stop rules of §4.1 step 4, in order.
    fn stop_check(&self) -> Option<Outcome> {
        let c = &self.cfg;
        if let Some((h, s_hi)) = self.hi {
            let close = self.lo.is_some_and(|(l, _)| h as i32 - l as i32 <= 1);
            if close || h == c.q_min || s_hi < c.target + c.score_slack {
                return self.best_found();
            }
        }
        if self.cap < c.q_min as i32 || self.lo.is_some_and(|(l, _)| l as i32 >= self.cap) {
            return Some(self.fallback());
        }
        if self.evals >= c.max_evals {
            return Some(self.fallback());
        }
        None
    }

    /// Passing candidate with the fewest bytes; ties go to the lower q (BTreeMap order).
    fn best_found(&self) -> Option<Outcome> {
        let mut best: Option<(u8, u64, f64)> = None;
        for (&q, e) in &self.cache {
            if let Some(s) = e.score {
                if s >= self.cfg.target && best.is_none_or(|(_, b, _)| e.bytes < b) {
                    best = Some((q, e.bytes, s));
                }
            }
        }
        best.map(|(q, bytes, score)| Outcome::Found {
            q,
            bytes,
            score,
            evals: self.evals,
        })
    }

    fn fallback(&self) -> Outcome {
        if let Some(o) = self.best_found() {
            return o;
        }
        if self.cache.values().any(|e| e.too_big) {
            return Outcome::Pruned { evals: self.evals };
        }
        // Highest score wins; ties go to the lower q. A NaN score never beats a number.
        let mut best: Option<(u8, f64)> = None;
        for (&q, e) in &self.cache {
            if let Some(s) = e.score {
                if best.is_none_or(|(_, b)| s > b || (b.is_nan() && !s.is_nan())) {
                    best = Some((q, s));
                }
            }
        }
        let (best_q, best_score) = best.unwrap_or((self.cfg.q_min, f64::NAN));
        Outcome::Unreachable {
            best_q,
            best_score,
            evals: self.evals,
        }
    }

    /// Clamp to `[q_min, cap]` (cap wins if it is below q_min).
    fn clamp(&self, q: i32) -> i32 {
        q.max(self.cfg.q_min as i32).min(self.cap)
    }

    fn is_cached(&self, q: i32) -> bool {
        u8::try_from(q).is_ok_and(|q| self.cache.contains_key(&q))
    }

    fn pick(&mut self) -> Option<u8> {
        let step = self.cfg.step as i32;
        let q = match (self.lo, self.hi) {
            (None, None) => self.clamp(self.last_q? as i32 - step),
            (Some((l, _)), None) => self.clamp(l as i32 + step),
            (None, Some((h, _))) => self.clamp(h as i32 - step),
            (Some((l, s_lo)), Some((h, s_hi))) => {
                let (l, h) = (l as i32, h as i32);
                let mid = self.clamp((l + h).div_euclid(2));
                let n = self.interp_sides.len();
                let same_side = n >= 2 && self.interp_sides[n - 1] == self.interp_sides[n - 2];
                let mut q =
                    if same_side || s_hi.partial_cmp(&s_lo) != Some(std::cmp::Ordering::Greater) {
                        mid
                    } else {
                        let t = (self.cfg.target - s_lo) / (s_hi - s_lo) * (h - l) as f64;
                        let raw = l + t.round() as i32;
                        self.clamp(raw.clamp(l + 1, (h - 1).max(l + 1)))
                    };
                if self.is_cached(q) {
                    q = mid;
                    if self.is_cached(q) {
                        return None;
                    }
                }
                self.pending_interp = true;
                return u8::try_from(q).ok();
            }
        };
        if self.is_cached(q) {
            return None;
        }
        u8::try_from(q).ok()
    }
}

/// Synchronous driver over Planner. The encode closure returns the byte length; the caller keeps the bytes.
pub fn search<E>(
    cfg: SearchConfig,
    size_bound: Option<u64>,
    mut encode: impl FnMut(u8) -> Result<u64, E>,
    mut score: impl FnMut(u8) -> Result<f64, E>,
) -> Result<Outcome, E> {
    let mut p = Planner::new(cfg, size_bound);
    loop {
        match p.next() {
            Step::Encode(q) => {
                let b = encode(q)?;
                p.on_encoded(q, b);
            }
            Step::Score(q) => {
                let s = score(q)?;
                p.on_scored(q, s);
            }
            Step::Finished(o) => return Ok(o),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Curve = fn(u8) -> f64;

    fn run(cfg: SearchConfig, bound: Option<u64>, score: Curve) -> (Outcome, Vec<Step>) {
        let mut p = Planner::new(cfg, bound);
        let mut steps = Vec::new();
        loop {
            let s = p.next();
            assert_eq!(p.next(), s, "next() must be idempotent");
            steps.push(s);
            match s {
                Step::Encode(q) => p.on_encoded(q, 1000 * q as u64),
                Step::Score(q) => p.on_scored(q, score(q)),
                Step::Finished(o) => return (o, steps),
            }
        }
    }

    fn encodes(steps: &[Step]) -> Vec<u8> {
        steps
            .iter()
            .filter_map(|s| match s {
                Step::Encode(q) => Some(*q),
                _ => None,
            })
            .collect()
    }

    fn scored(steps: &[Step]) -> Vec<u8> {
        steps
            .iter()
            .filter_map(|s| match s {
                Step::Score(q) => Some(*q),
                _ => None,
            })
            .collect()
    }

    fn sigmoid(q: u8) -> f64 {
        100.0 / (1.0 + (-(q as f64 - 70.0) / 6.0).exp())
    }

    fn nonmono(q: u8) -> f64 {
        q as f64 - 10.0 + 3.0 * (q as f64).sin()
    }

    fn cases() -> Vec<(SearchConfig, Option<u64>, Curve)> {
        vec![
            (SearchConfig::new(80.0, 60), None, |q| q as f64 - 10.0),
            (SearchConfig::new(80.0, 50), None, sigmoid),
            (SearchConfig::new(80.0, 90), None, |q| 0.9 * q as f64),
            (SearchConfig::new(80.0, 80), None, |q| q as f64 / 2.0),
            (SearchConfig::new(80.0, 60), Some(50_000), |q| {
                q as f64 + 20.0
            }),
            (SearchConfig::new(80.0, 85), None, nonmono),
        ]
    }

    #[test]
    fn a_linear() {
        let (cfg, b, f) = cases()[0];
        let (o, steps) = run(cfg, b, f);
        assert_eq!(encodes(&steps), vec![60, 68, 76, 84, 92, 90]);
        assert!(
            matches!(
                o,
                Outcome::Found {
                    q: 90,
                    evals: 6,
                    ..
                }
            ),
            "{o:?}"
        );
    }

    #[test]
    fn b_sigmoid() {
        let (cfg, b, f) = cases()[1];
        let (o, steps) = run(cfg, b, f);
        let true_min = (1..=100).find(|&q| sigmoid(q) >= 80.0).unwrap();
        assert_eq!(true_min, 79);
        match o {
            Outcome::Found { q, evals, .. } => {
                assert_eq!(q, true_min, "{steps:?}");
                assert!(evals <= 6);
            }
            _ => panic!("{o:?}"),
        }
    }

    #[test]
    fn c_slack() {
        let (cfg, b, f) = cases()[2];
        let (o, steps) = run(cfg, b, f);
        assert_eq!(encodes(&steps), vec![90, 82, 89]);
        assert!(matches!(o, Outcome::Found { q: 89, .. }), "{o:?}");
    }

    #[test]
    fn d_unreachable() {
        let (cfg, b, f) = cases()[3];
        let (o, _) = run(cfg, b, f);
        assert!(
            matches!(
                o,
                Outcome::Unreachable {
                    best_q: 100,
                    evals: 4,
                    ..
                }
            ),
            "{o:?}"
        );
    }

    #[test]
    fn e_pruned() {
        let (cfg, b, f) = cases()[4];
        let (o, steps) = run(cfg, b, f);
        match o {
            Outcome::Pruned { evals } => assert!(evals <= 6),
            _ => panic!("{o:?}"),
        }
        for q in scored(&steps) {
            assert!(1000 * (q as u64) < 50_000, "scored too-big q {q}");
        }
    }

    #[test]
    fn f_non_monotone() {
        let (cfg, b, f) = cases()[5];
        let (o, steps) = run(cfg, b, f);
        let best = scored(&steps)
            .into_iter()
            .filter(|&q| nonmono(q) >= 80.0)
            .min_by_key(|&q| 1000 * q as u64)
            .unwrap();
        match o {
            Outcome::Found {
                q, score, bytes, ..
            } => {
                assert!(score >= 80.0);
                assert_eq!(q, best);
                assert_eq!(bytes, 1000 * q as u64);
            }
            _ => panic!("{o:?}"),
        }
    }

    #[test]
    fn g_determinism() {
        for (cfg, b, f) in cases() {
            assert_eq!(run(cfg, b, f), run(cfg, b, f));
        }
    }

    #[test]
    fn h_driver_matches_manual() {
        for (cfg, b, f) in cases() {
            let got: Result<Outcome, ()> = search(cfg, b, |q| Ok(1000 * q as u64), |q| Ok(f(q)));
            assert_eq!(got.unwrap(), run(cfg, b, f).0);
        }
    }

    #[test]
    fn driver_propagates_errors() {
        let r: Result<Outcome, &str> = search(
            SearchConfig::new(80.0, 50),
            None,
            |_| Err("boom"),
            |_| Ok(0.0),
        );
        assert_eq!(r, Err("boom"));
    }

    /// Seeded LCG (PCG multiplier), so the property test needs no dependency.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        /// Uniform in [0, 1).
        fn unit(&mut self) -> f64 {
            self.next() as f64 / (1u64 << 31) as f64
        }
        fn range(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (hi - lo) * self.unit()
        }
    }

    /// Score and byte tables indexed by q (index 0 unused).
    struct Table {
        score: [f64; 101],
        bytes: [u64; 101],
        /// Both score and bytes are non-decreasing in q.
        monotone: bool,
    }

    fn random_table(r: &mut Lcg) -> Table {
        let mut score = [0.0f64; 101];
        let kind = r.below(7);
        match kind {
            // Linear.
            0 => {
                let (a, b) = (r.range(0.05, 2.0), r.range(-60.0, 70.0));
                for (q, v) in score.iter_mut().enumerate().skip(1) {
                    *v = a * q as f64 + b;
                }
            }
            // Sigmoid.
            1 => {
                let (c, w) = (r.range(10.0, 100.0), r.range(1.0, 20.0));
                for (q, v) in score.iter_mut().enumerate().skip(1) {
                    *v = 100.0 / (1.0 + (-(q as f64 - c) / w).exp());
                }
            }
            // Random non-decreasing steps with plateaus.
            2 => {
                let mut s = r.range(-20.0, 60.0);
                for v in score.iter_mut().skip(1) {
                    if r.below(3) != 0 {
                        s += r.range(0.0, 3.0);
                    }
                    *v = s;
                }
            }
            // Concave power curve that saturates near 100.
            3 => {
                let (k, p) = (r.range(0.001, 0.5), r.range(1.0, 2.5));
                for (q, v) in score.iter_mut().enumerate().skip(1) {
                    *v = 100.0 - k * ((101 - q) as f64).powf(p);
                }
            }
            // Monotone base plus noise.
            4 | 5 => {
                let (a, b, amp) = (r.range(0.2, 1.5), r.range(-20.0, 40.0), r.range(0.3, 6.0));
                for (q, v) in score.iter_mut().enumerate().skip(1) {
                    *v = a * q as f64 + b + amp * (r.unit() * 2.0 - 1.0);
                }
            }
            // Arbitrary.
            _ => {
                for v in score.iter_mut().skip(1) {
                    *v = r.range(0.0, 100.0);
                }
            }
        }
        let mut bytes = [0u64; 101];
        let bytes_monotone = r.below(8) != 0;
        let mut b = 100 + r.below(5_000);
        for v in bytes.iter_mut().skip(1) {
            if bytes_monotone {
                b += r.below(2_000); // 0 allowed: plateaus
                *v = b;
            } else {
                *v = 100 + r.below(200_000);
            }
        }
        let score_monotone = score[1..].windows(2).all(|w| w[0] <= w[1]);
        Table {
            score,
            bytes,
            monotone: score_monotone && bytes_monotone,
        }
    }

    #[derive(Default, Debug)]
    struct Stats {
        runs: u32,
        found: u32,
        unreachable: u32,
        pruned: u32,
        monotone_found: u32,
        monotone_exact: u32,
        monotone_gap_slack: u32,
        monotone_gap_budget: u32,
        max_gap_slack: u8,
        max_gap_budget: u8,
        monotone_false_unreachable: u32,
        monotone_false_pruned: u32,
        max_evals_seen: u8,
    }

    /// Steps one search by hand and checks every invariant; returns the outcome.
    fn check_one(cfg: SearchConfig, bound: Option<u64>, t: &Table, st: &mut Stats) {
        let too_big = |q: u8| bound.is_some_and(|b| t.bytes[q as usize] >= b);
        let mut p = Planner::new(cfg, bound);
        let mut encoded: BTreeMap<u8, u64> = BTreeMap::new();
        let mut scored: BTreeMap<u8, f64> = BTreeMap::new();
        let ctx = format!("{cfg:?} bound={bound:?}");
        let outcome = loop {
            let s = p.next();
            // Compared via Debug because an Unreachable best_score may be NaN.
            assert_eq!(
                format!("{:?}", p.next()),
                format!("{s:?}"),
                "next() not idempotent: {ctx}"
            );
            match s {
                Step::Encode(q) => {
                    assert!(
                        (cfg.q_min..=cfg.q_max).contains(&q),
                        "q {q} out of range: {ctx}"
                    );
                    assert!(!encoded.contains_key(&q), "q {q} encoded twice: {ctx}");
                    assert!(
                        encoded.len() < cfg.max_evals as usize,
                        "more than max_evals encodes: {ctx}"
                    );
                    encoded.insert(q, t.bytes[q as usize]);
                    p.on_encoded(q, t.bytes[q as usize]);
                    // An encode that fits must be scored next, before anything else.
                    let n = p.next();
                    if too_big(q) {
                        assert!(!matches!(n, Step::Score(_)), "too-big q {q} scored: {ctx}");
                    } else {
                        assert_eq!(n, Step::Score(q), "{ctx}");
                    }
                }
                Step::Score(q) => {
                    assert!(!too_big(q), "scored too-big q {q}: {ctx}");
                    assert!(scored.insert(q, t.score[q as usize]).is_none(), "{ctx}");
                    p.on_scored(q, t.score[q as usize]);
                }
                Step::Finished(o) => break o,
            }
        };
        st.runs += 1;
        let evals = encoded.len() as u8;
        st.max_evals_seen = st.max_evals_seen.max(evals);
        let passing = || scored.iter().filter(|(_, &s)| s >= cfg.target);
        let any_too_big = encoded.keys().any(|&q| too_big(q));
        // Truly best achievable candidate on this curve: lowest q that passes and fits.
        let true_min =
            (cfg.q_min..=cfg.q_max).find(|&q| t.score[q as usize] >= cfg.target && !too_big(q));
        match outcome {
            Outcome::Found {
                q,
                bytes,
                score,
                evals: e,
            } => {
                st.found += 1;
                assert!(score >= cfg.target, "Found below target: {ctx}");
                assert_eq!(e, evals, "{ctx}");
                assert_eq!(
                    scored.get(&q),
                    Some(&score),
                    "Found q was not scored: {ctx}"
                );
                assert_eq!(bytes, t.bytes[q as usize], "{ctx}");
                let best = passing()
                    .map(|(&q, _)| (t.bytes[q as usize], q))
                    .min()
                    .unwrap();
                assert_eq!((bytes, q), best, "not the smallest passing: {ctx}");
                if t.monotone {
                    st.monotone_found += 1;
                    let m = true_min.expect("a pass was scored, so one exists");
                    assert!(q >= m, "{ctx}");
                    let gap = q - m;
                    if gap == 0 {
                        st.monotone_exact += 1;
                    } else if score < cfg.target + cfg.score_slack {
                        st.monotone_gap_slack += 1;
                        st.max_gap_slack = st.max_gap_slack.max(gap);
                    } else {
                        // The only other way to stop above the minimum is running out of evals.
                        assert_eq!(
                            evals, cfg.max_evals,
                            "gap {gap} without slack or budget: {ctx}"
                        );
                        st.monotone_gap_budget += 1;
                        st.max_gap_budget = st.max_gap_budget.max(gap);
                    }
                }
            }
            Outcome::Unreachable {
                best_q,
                best_score,
                evals: e,
            } => {
                st.unreachable += 1;
                assert_eq!(e, evals, "{ctx}");
                assert_eq!(passing().count(), 0, "{ctx}");
                assert!(!any_too_big, "{ctx}");
                // Highest non-NaN score, ties to the lower q; NaN only if every score is NaN.
                let mut want: Option<(u8, f64)> = None;
                for (&q, &s) in &scored {
                    if want.is_none_or(|(_, b)| s > b || (b.is_nan() && !s.is_nan())) {
                        want = Some((q, s));
                    }
                }
                let (wq, ws) = want.expect("an Unreachable search scored something");
                assert_eq!((best_q, best_score.to_bits()), (wq, ws.to_bits()), "{ctx}");
                if t.monotone && true_min.is_some() {
                    assert_eq!(evals, cfg.max_evals, "false Unreachable: {ctx}");
                    st.monotone_false_unreachable += 1;
                }
            }
            Outcome::Pruned { evals: e } => {
                st.pruned += 1;
                assert_eq!(e, evals, "{ctx}");
                assert_eq!(passing().count(), 0, "{ctx}");
                assert!(any_too_big, "{ctx}");
                if t.monotone && true_min.is_some() {
                    assert_eq!(evals, cfg.max_evals, "false Pruned: {ctx}");
                    st.monotone_false_pruned += 1;
                }
            }
        }
    }

    #[test]
    fn property_random_curves() {
        let mut r = Lcg(0x5EED_C0DE);
        let mut st = Stats::default();
        for _ in 0..20_000 {
            let t = random_table(&mut r);
            let target = r.range(20.0, 99.0);
            let initial = r.below(110) as u8; // includes 0 and > 100 to exercise clamping
            let mut cfg = SearchConfig::new(target, initial);
            if r.below(4) == 0 {
                cfg.max_evals = 1 + r.below(12) as u8;
                cfg.step = 1 + r.below(20) as u8;
            }
            let bound = match r.below(3) {
                0 => None,
                _ => Some(t.bytes[1 + r.below(100) as usize] + r.below(3)),
            };
            let mut t = t;
            if r.below(50) == 0 {
                // Degenerate scores must not panic or loop.
                t.score[1 + r.below(100) as usize] =
                    [f64::NAN, f64::INFINITY, f64::NEG_INFINITY][r.below(3) as usize];
                t.monotone = false;
            }
            check_one(cfg, bound, &t, &mut st);
            // Same search through the driver gives the same outcome.
            let via_driver: Result<Outcome, ()> = search(
                cfg,
                bound,
                |q| Ok(t.bytes[q as usize]),
                |q| Ok(t.score[q as usize]),
            );
            let mut p = Planner::new(cfg, bound);
            let manual = loop {
                match p.next() {
                    Step::Encode(q) => p.on_encoded(q, t.bytes[q as usize]),
                    Step::Score(q) => p.on_scored(q, t.score[q as usize]),
                    Step::Finished(o) => break o,
                }
            };
            assert_eq!(format!("{:?}", via_driver.unwrap()), format!("{manual:?}"));
        }
        eprintln!("{st:#?}");
        assert!(
            st.found > 1000 && st.unreachable > 100 && st.pruned > 100,
            "{st:?}"
        );
        assert!(st.monotone_exact > 1000, "{st:?}");
    }

    /// Default config on monotone curves with a prior within two steps (±16) of the true
    /// minimal passing q. Measured with this seed: 96.8% exact, 99.2% within 1 q, mean
    /// 4.0 evals; every larger gap comes from the +0.5 slack stop on a flat curve.
    #[test]
    fn property_realistic_priors() {
        let mut r = Lcg(0xBADC_0FFE);
        let mut st = Stats::default();
        let mut within_one = 0u32;
        let mut total_evals = 0u32;
        while st.runs < 20_000 {
            let t = random_table(&mut r);
            if !t.monotone {
                continue;
            }
            let target = r.range(40.0, 95.0);
            let Some(m) = (1..=100u8).find(|&q| t.score[q as usize] >= target) else {
                continue;
            };
            let initial = (m as i32 + r.below(33) as i32 - 16).clamp(1, 100) as u8;
            let cfg = SearchConfig::new(target, initial);
            check_one(cfg, None, &t, &mut st);
            let o = search::<()>(
                cfg,
                None,
                |q| Ok(t.bytes[q as usize]),
                |q| Ok(t.score[q as usize]),
            )
            .unwrap();
            let Outcome::Found { q, evals, .. } = o else {
                panic!("reachable curve not found: {o:?} {cfg:?}");
            };
            within_one += u32::from(q - m <= 1);
            total_evals += u32::from(evals);
        }
        eprintln!(
            "{st:#?}\nwithin 1 q: {within_one}, mean evals {:.2}",
            total_evals as f64 / st.runs as f64
        );
        assert_eq!(st.found, st.runs, "{st:?}");
        assert!(st.max_gap_budget <= 8, "{st:?}");
        assert!(st.monotone_exact * 100 >= st.runs * 95, "{st:?}");
        assert!(within_one * 1000 >= st.runs * 985, "{st:?}");
    }

    /// §4.1 same-side safeguard: after two both-bounds picks land on the same side the
    /// planner bisects, and a bisection pick counts as a pick for the next check.
    #[test]
    fn same_side_safeguard_counts_midpoints() {
        fn concave(q: u8) -> f64 {
            let x = ((q as f64 - 20.0) / 80.0).clamp(0.0, 1.0);
            100.0 * (1.0 - (1.0 - x).powi(4))
        }
        let mut cfg = SearchConfig::new(80.0, 100);
        cfg.step = 80;
        cfg.max_evals = 9;
        let (o, steps) = run(cfg, None, concave);
        // 84 and 71 both pass, so 45 is the midpoint of (20, 71); it fails, which breaks the
        // run of same-side picks, so 48 is interpolated again. 48 and 47 both pass: 46 bisects.
        assert_eq!(encodes(&steps), vec![100, 20, 84, 71, 45, 48, 47, 46]);
        assert!(matches!(o, Outcome::Found { q: 47, .. }), "{o:?}");
        assert_eq!((1..=100).find(|&q| concave(q) >= 80.0), Some(47));
    }

    #[test]
    #[should_panic]
    fn mismatched_on_encoded_panics() {
        let mut p = Planner::new(SearchConfig::new(80.0, 50), None);
        let _ = p.next();
        p.on_encoded(51, 1);
    }

    #[test]
    #[should_panic]
    fn mismatched_on_scored_panics() {
        let mut p = Planner::new(SearchConfig::new(80.0, 50), None);
        let _ = p.next();
        p.on_scored(50, 1.0);
    }
}
