//! Vulkan WSI presentation via VkSwapchainKHR.
//!
//! Surface from raw-window-handle through ash-window. FIFO by default, MAILBOX
//! when low latency is requested and available; SUBOPTIMAL and OUT_OF_DATE are
//! handled by reconfigure.
