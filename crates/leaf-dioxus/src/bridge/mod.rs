//! The bridge from leaf's rows to Dioxus's nodes: glyphs coalesced into runs
//! ([`rows`]), a run's style as CSS classes ([`style`]), and the frame delta
//! that lets a keystroke render one row and reuse the rest ([`frame`]).
//!
//! None of it names a Dioxus type. It began in the virtual-DOM spike
//! (`spikes/dioxus-widget`), which keeps its own copy for its measurements.

pub mod frame;
pub mod rows;
pub mod style;
