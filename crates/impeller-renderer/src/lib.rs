//! Render pass encoding, generic over the HAL. DrawCommands are buffered per
//! pass, sorted by pipeline, and encoded once at pass end.
