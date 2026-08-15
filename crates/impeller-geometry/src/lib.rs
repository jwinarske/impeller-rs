//! Path types and tessellation.
//!
//! Two strategies, chosen per shape. General fills and strokes go through
//! tessellation. Rects, rounded rects, circles, and ellipses take analytic
//! fast paths that compute coverage in the fragment shader instead, skipping
//! tessellation entirely — that is where most of the draw calls in a real UI
//! land.
//!
//! Convex paths take a fan-fill fast path that avoids the stencil pass, so
//! [`path::Path::convexity`] is worth consulting before choosing a strategy.
//! Detection is conservative: a wrong `Convex` answer renders incorrectly,
//! while a wrong `Concave` answer only costs speed.

pub mod flatten;
pub mod path;

pub use flatten::{flatten, DEFAULT_TOLERANCE};
pub use path::{Convexity, FillRule, Path, PathBuilder, Rect, Verb};
