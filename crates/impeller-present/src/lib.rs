//! Presentation trait and shared types.
//!
//! The presentation axis is orthogonal to the rendering HAL: the HAL answers
//! how draw commands become pixels in a GPU image, and presentation answers how
//! a finished image reaches the display and how the frame loop is paced.
//! Presentation owns pacing -- WSI paces via swapchain acquire semantics, DRM
//! via page-flip completion events.
//!
//! Also owns format and modifier negotiation. A negotiation failure is a hard
//! error with both sides' sets dumped, and the chosen modifier is always
//! logged: a silent linear fallback halves memory bandwidth on an embedded
//! panel, so it is treated as a bug rather than a graceful degradation.
