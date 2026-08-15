//! GLES 3.0 backend via glow, with contexts from EGL in all configurations.
//!
//! Commands are recorded into a vec and replayed as GL calls at submit, with a
//! state cache to avoid redundant binds. This costs CPU relative to Vulkan and
//! is the accepted trade for keeping the HAL trait explicit.
