//! Shared test harness.
//!
//! One corpus, many executions. Scenes are data rather than Rust code, so a
//! single corpus drives golden comparison, cross-backend conformance,
//! performance runs, and on-device runs without being rewritten for each. A new
//! feature adds scenes once and every execution mode picks them up, which is
//! what keeps authoring cost flat while the test matrix multiplies.
//!
//! The pieces:
//!
//! - [`shape`] and [`scene`] describe what to draw, as plain data.
//! - [`executor`] renders a scene through any backend implementing the HAL.
//! - [`image`] compares results, with tolerances that state where the
//!   specification permits a difference rather than papering over one.

pub mod executor;
pub mod image;
pub mod scene;
pub mod shape;

pub use executor::{pass_for, record_scene, render_corpus, render_scene};
pub use image::{accepts, compare, Difference, Image, Tolerance};
pub use scene::{corpus, Fill, Item, Scene, Stop, StrokeSpec, Transform};
pub use shape::Shape;
