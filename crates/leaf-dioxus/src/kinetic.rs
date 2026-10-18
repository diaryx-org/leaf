//! Scrolling as a phone's own scroll view does it: the finger's speed carried
//! on when it lets go and slowing as UIKit's `UIScrollView` slows, a stretch
//! past either end that gives less the further it is pulled, and a spring
//! back to the end once the finger is gone.
//!
//! The numbers are UIKit's: a scroll view's normal deceleration rate is
//! 0.998 — the speed kept per millisecond — and its rubber band gives
//! `(1 − 1 / (x·0.55 / d + 1))·d` for a pull of `x` past an end of a view
//! `d` tall. The spring back is critically damped, so it settles without a
//! wobble, in about a third of a second.
//!
//! Pure arithmetic over positions and milliseconds, so that a frame that
//! comes late moves the page as far as an on-time one would have.

use std::collections::VecDeque;
use std::time::Instant;

/// The speed kept per millisecond (`UIScrollView.DecelerationRate.normal`).
pub const DECELERATION: f64 = 0.998;

/// How stiff the rubber band is (UIKit's constant).
const RUBBER: f64 = 0.55;

/// How quickly the spring back settles, per millisecond: about a third of a
/// second from a full stretch.
const SPRING: f64 = 0.012;

/// Below this speed, in CSS pixels a millisecond, a glide has stopped.
const STOPPED: f64 = 0.01;

/// How far back the finger's speed is read from, in milliseconds: the last
/// stretch of the drag, as UIKit reads it.
const WINDOW: f64 = 80.0;

/// How long a finger can rest before it lets go and still throw the page:
/// held still longer than this, it lets go of a page that is not moving.
const REST: f64 = 50.0;

/// Where a page pulled to `raw` shows, for a view `d` tall that may go from
/// `min` to `max`: `raw` itself between them, and past either, the rubber
/// band's share of the pull.
pub fn rubber(raw: f64, min: f64, max: f64, d: f64) -> f64 {
    let give = |x: f64| (1.0 - 1.0 / (x * RUBBER / d.max(1.0) + 1.0)) * d.max(1.0);
    if raw < min {
        min - give(min - raw)
    } else if raw > max {
        max + give(raw - max)
    } else {
        raw
    }
}

/// The finger's recent positions, for its speed when it lets go.
#[derive(Default, Debug)]
pub struct Tracker {
    samples: VecDeque<(Instant, f64)>,
}

impl Tracker {
    /// Begin again: a finger went down.
    pub fn reset(&mut self) {
        self.samples.clear();
    }

    /// The finger is at `y` now.
    pub fn push(&mut self, y: f64) {
        let now = Instant::now();
        self.samples.push_back((now, y));
        while self
            .samples
            .front()
            .is_some_and(|(t, _)| ms(now.duration_since(*t)) > WINDOW)
        {
            self.samples.pop_front();
        }
    }

    /// The finger's speed as it lets go, in CSS pixels a millisecond: over
    /// the last stretch of the drag, or nothing if it had come to rest.
    pub fn velocity(&self) -> f64 {
        let now = Instant::now();
        let (Some(&(t0, y0)), Some(&(t1, y1))) = (self.samples.front(), self.samples.back()) else {
            return 0.0;
        };
        if ms(now.duration_since(t1)) > REST {
            return 0.0;
        }
        let span = ms(t1.duration_since(t0));
        if span < 1.0 {
            return 0.0;
        }
        (y1 - y0) / span
    }
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// A page moving on its own: gliding, slowing, and — past an end — sprung
/// back to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glide {
    /// Where the page is, and its speed in CSS pixels a millisecond.
    pub pos: f64,
    pub vel: f64,
    /// The ends it may rest between.
    pub min: f64,
    pub max: f64,
}

impl Glide {
    /// Move it on by `dt` milliseconds. Whether it is still moving.
    pub fn step(&mut self, dt: f64) -> bool {
        let dt = dt.clamp(0.0, 100.0);
        let edge = if self.pos < self.min {
            Some(self.min)
        } else if self.pos > self.max {
            Some(self.max)
        } else {
            None
        };
        match edge {
            // Between the ends: the speed decays exponentially, and the page
            // goes as far as that speed carries it over `dt`.
            None => {
                let k = DECELERATION.ln();
                let kept = DECELERATION.powf(dt);
                self.pos += self.vel * (kept - 1.0) / k;
                self.vel *= kept;
                if self.vel.abs() < STOPPED {
                    self.vel = 0.0;
                    self.pos = self.pos.clamp(self.min, self.max);
                    return false;
                }
                true
            }
            // Past one: a critically damped spring to it, which takes the
            // speed it came with and turns it round.
            Some(edge) => {
                let x0 = self.pos - edge;
                let v0 = self.vel;
                let w = SPRING;
                let e = (-w * dt).exp();
                let x = (x0 + (v0 + w * x0) * dt) * e;
                let v = (v0 - w * dt * (v0 + w * x0)) * e;
                self.pos = edge + x;
                self.vel = v;
                if x.abs() < 0.5 && v.abs() < STOPPED {
                    self.pos = edge;
                    self.vel = 0.0;
                    return false;
                }
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(mut g: Glide) -> (Glide, f64) {
        let mut t = 0.0;
        while g.step(8.0) {
            t += 8.0;
            assert!(t < 10_000.0, "a glide ends");
        }
        (g, t)
    }

    #[test]
    fn a_throw_travels_as_far_as_uikits() {
        // UIKit's distance for a throw at v: v·d / (1 − d), with d per ms.
        let v = 2.0;
        let (g, t) = run(Glide {
            pos: 0.0,
            vel: v,
            min: 0.0,
            max: 1e9,
        });
        let expected = v * DECELERATION / (1.0 - DECELERATION);
        assert!(
            (g.pos - expected).abs() < expected * 0.02,
            "{} vs {expected}",
            g.pos
        );
        assert!(t > 1000.0 && t < 4000.0, "about two seconds: {t}");
    }

    #[test]
    fn a_frame_late_goes_as_far_as_two_on_time() {
        let mut a = Glide {
            pos: 0.0,
            vel: 1.0,
            min: 0.0,
            max: 1e9,
        };
        let mut b = a;
        a.step(16.0);
        b.step(8.0);
        b.step(8.0);
        assert!((a.pos - b.pos).abs() < 1e-9);
    }

    #[test]
    fn a_throw_past_the_end_springs_back_to_it() {
        let (g, _) = run(Glide {
            pos: 990.0,
            vel: 3.0,
            min: 0.0,
            max: 1000.0,
        });
        assert_eq!(g.pos, 1000.0);
        let (g, _) = run(Glide {
            pos: -60.0,
            vel: 0.0,
            min: 0.0,
            max: 1000.0,
        });
        assert_eq!(g.pos, 0.0);
    }

    #[test]
    fn the_rubber_band_gives_less_the_further_it_is_pulled() {
        let d = 800.0;
        assert_eq!(rubber(400.0, 0.0, 1000.0, d), 400.0);
        let a = rubber(-100.0, 0.0, 1000.0, d);
        let b = rubber(-200.0, 0.0, 1000.0, d);
        assert!(a < 0.0 && a > -100.0);
        assert!(b < a && (b - a).abs() < 100.0);
        assert!(
            rubber(1e6, 0.0, 1000.0, d) < 1000.0 + d,
            "never past a view's height"
        );
    }
}
