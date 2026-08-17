//! EGL context creation and capability detection.
//!
//! Contexts come from EGL in every configuration, including this headless one.
//! That is deliberate: the same code path serves a window surface, a GBM
//! surface for direct scanout, and the surfaceless context tests run on, so
//! there is no test-only path that could drift from the one that ships.

use glow::HasContext;
use impeller_hal::{Capabilities, DmaBufSupport, Error, Result, SampleCounts, SyncSupport};
use std::collections::HashSet;
use std::sync::Mutex;

/// EGL 1.5 rather than 1.4, because platform displays are what select the
/// surfaceless, GBM, or window platform explicitly. The 1.4 entry point guesses
/// from the native display type, which is exactly the ambiguity the DRM path
/// cannot afford.
type Egl = khronos_egl::DynamicInstance<khronos_egl::EGL1_5>;

/// `EGL_PLATFORM_SURFACELESS_MESA`, for a context with no drawable at all.
const PLATFORM_SURFACELESS: khronos_egl::Enum = 0x31DD;

/// Extensions that decide which DRM presentation path is available.
mod ext {
    /// Import a dma-buf as an EGLImage, then bind it as a texture.
    pub const DMA_BUF_IMPORT: &str = "EGL_EXT_image_dma_buf_import";
    /// Export an EGLImage back out as a dma-buf.
    pub const DMA_BUF_EXPORT: &str = "EGL_MESA_image_dma_buf_export";
    /// Query the modifiers a driver accepts, rather than assuming linear.
    pub const DMA_BUF_MODIFIERS: &str = "EGL_EXT_image_dma_buf_import_modifiers";
    /// Turn a GL fence into a sync_file fd, which is what keeps the DRM frame
    /// loop explicit rather than falling back to a CPU wait.
    pub const NATIVE_FENCE_SYNC: &str = "EGL_ANDROID_native_fence_sync";
}

/// How the context gets its display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayTarget {
    /// No drawable. Rendering goes to framebuffer objects, which is all the
    /// offscreen executor and the golden corpus need.
    #[default]
    Surfaceless,
}

/// A GLES context and what it can do.
pub struct GlesContext {
    gl: glow::Context,
    egl: Egl,
    display: khronos_egl::Display,
    context: khronos_egl::Context,
    capabilities: Capabilities,
    egl_extensions: HashSet<String>,
    gl_extensions: HashSet<String>,
    program: Option<crate::render::SolidProgram>,
    /// A one-pixel opaque white texture, bound where a draw samples nothing.
    ///
    /// Created on first use rather than eagerly, so a context that only ever
    /// fills shapes allocates nothing for a feature it does not use. White
    /// rather than transparent so that binding it in place of a real texture
    /// shows up as a blank shape rather than as nothing at all.
    placeholder: Option<glow::Texture>,
}

impl GlesContext {
    pub fn new(target: DisplayTarget) -> Result<Self> {
        // SAFETY: loads libEGL by soname and keeps it alive for the lifetime of
        // the returned instance, which this struct owns.
        let egl = unsafe { Egl::load_required() }.map_err(|e| Error::Backend {
            backend: "gles",
            detail: format!("loading libEGL with EGL 1.5 entry points: {e}"),
        })?;

        let client_extensions = egl
            .query_string(None, khronos_egl::EXTENSIONS)
            .map(|s| split_extensions(&s.to_string_lossy()))
            .unwrap_or_default();

        let display = match target {
            DisplayTarget::Surfaceless => {
                if !client_extensions.contains("EGL_MESA_platform_surfaceless") {
                    return Err(Error::Unsupported("EGL_MESA_platform_surfaceless"));
                }
                // SAFETY: the null display argument is what the surfaceless
                // platform defines as its default, and the attribute list is
                // terminated.
                unsafe {
                    egl.get_platform_display(
                        PLATFORM_SURFACELESS,
                        khronos_egl::DEFAULT_DISPLAY,
                        &[khronos_egl::ATTRIB_NONE],
                    )
                }
                .map_err(|e| backend_err("get_platform_display", e))?
            }
        };

        acquire_display(&egl, display)?;

        let egl_extensions = egl
            .query_string(Some(display), khronos_egl::EXTENSIONS)
            .map(|s| split_extensions(&s.to_string_lossy()))
            .unwrap_or_default();

        // A surfaceless context still needs a config; it simply never gets a
        // surface bound to it.
        if !egl_extensions.contains("EGL_KHR_surfaceless_context") {
            release_display(&egl, display);
            return Err(Error::Unsupported("EGL_KHR_surfaceless_context"));
        }

        egl.bind_api(khronos_egl::OPENGL_ES_API)
            .map_err(|e| backend_err("bind_api", e))?;

        let config_attrs = [
            khronos_egl::SURFACE_TYPE,
            khronos_egl::PBUFFER_BIT,
            khronos_egl::RENDERABLE_TYPE,
            khronos_egl::OPENGL_ES3_BIT,
            khronos_egl::RED_SIZE,
            8,
            khronos_egl::GREEN_SIZE,
            8,
            khronos_egl::BLUE_SIZE,
            8,
            khronos_egl::ALPHA_SIZE,
            8,
            khronos_egl::NONE,
        ];
        let config = match egl.choose_first_config(display, &config_attrs) {
            Ok(Some(config)) => config,
            Ok(None) => {
                release_display(&egl, display);
                return Err(Error::Unsupported("no EGL config with an ES3 RGBA8 target"));
            }
            Err(e) => {
                release_display(&egl, display);
                return Err(backend_err("choose_first_config", e));
            }
        };

        // ES 3.0 is the floor the project targets. A driver offering more
        // reports it through GL_VERSION and features are used from there.
        let context_attrs = [
            khronos_egl::CONTEXT_MAJOR_VERSION,
            3,
            khronos_egl::CONTEXT_MINOR_VERSION,
            0,
            khronos_egl::NONE,
        ];
        let context = match egl.create_context(display, config, None, &context_attrs) {
            Ok(c) => c,
            Err(e) => {
                release_display(&egl, display);
                return Err(backend_err("create_context", e));
            }
        };

        if let Err(e) = egl.make_current(display, None, None, Some(context)) {
            let _ = egl.destroy_context(display, context);
            release_display(&egl, display);
            return Err(backend_err("make_current", e));
        }

        // SAFETY: a context is current on this thread, so entry points resolved
        // here are valid for it.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name)
                    .map_or(std::ptr::null(), |p| p as *const std::ffi::c_void)
            })
        };

        let gl_extensions = gl_extension_set(&gl);
        let capabilities = detect_capabilities(&gl, &egl_extensions);

        Ok(Self {
            gl,
            egl,
            display,
            context,
            capabilities,
            egl_extensions,
            gl_extensions,
            program: None,
            placeholder: None,
        })
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// A one-pixel opaque white texture, for draws that sample nothing.
    pub(crate) fn placeholder_texture(&mut self) -> Result<glow::Texture> {
        if let Some(texture) = self.placeholder {
            return Ok(texture);
        }
        let gl = self.raw_gl();
        // SAFETY: a context is current, and the source is sized for the one
        // pixel being written.
        let texture = unsafe {
            let texture = gl
                .create_texture()
                .map_err(|e| crate::render::gl_err("create_texture", &e))?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                1,
                1,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                Some(&[255u8, 255, 255, 255]),
            );
            // Filtering must be set explicitly: a texture with the default
            // mipmap filter and no mipmaps is incomplete and samples as black,
            // which would make the placeholder do exactly what it exists to
            // avoid.
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            for axis in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, axis, glow::CLAMP_TO_EDGE as i32);
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            texture
        };
        self.placeholder = Some(texture);
        Ok(texture)
    }

    pub(crate) fn program(&self) -> Option<&crate::render::SolidProgram> {
        self.program.as_ref()
    }

    pub(crate) fn set_program(&mut self, program: crate::render::SolidProgram) {
        self.program = Some(program);
    }

    pub fn raw_gl(&self) -> &glow::Context {
        &self.gl
    }

    pub fn has_egl_extension(&self, name: &str) -> bool {
        self.egl_extensions.contains(name)
    }

    /// Whether a GL-side extension is present.
    ///
    /// Kept queryable rather than folded into [`Capabilities`] until something
    /// acts on one. `GL_EXT_multisampled_render_to_texture` is the first that
    /// will: on a tiler it resolves inside tile memory and needs no separate
    /// multisample buffer, which changes how a multisampled pass is built
    /// rather than only how fast it runs.
    pub fn has_gl_extension(&self, name: &str) -> bool {
        self.gl_extensions.contains(name)
    }
}

impl Drop for GlesContext {
    fn drop(&mut self) {
        if let Some(program) = self.program.take() {
            use glow::HasContext;
            // SAFETY: the context is still current here, and nothing else holds
            // these objects.
            unsafe {
                self.gl.delete_program(program.program);
                self.gl.delete_vertex_array(program.vao);
                self.gl.delete_buffer(program.vertices);
                self.gl.delete_buffer(program.indices);
            }
        }
        // Unbind before destroying, or the driver keeps the context alive and
        // the display never actually releases its resources.
        let _ = self.egl.make_current(self.display, None, None, None);
        let _ = self.egl.destroy_context(self.display, self.context);

        // Terminating is refcounted across contexts, because an EGL display is
        // process-global: asking for the same platform twice returns the same
        // handle, and terminating it invalidates every context on it, not just
        // this one. Doing it unconditionally tore down contexts belonging to
        // other live users, which showed up as a second context reporting that
        // the display had no extensions at all.
        release_display(&self.egl, self.display);
    }
}

/// How many live contexts each process-global display has.
///
/// A count rather than a flag: the last user out is the one that may terminate,
/// and a flag could not tell the difference between one user and several.
static DISPLAY_USERS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// Record a new user of a display, initializing it on first use.
fn acquire_display(egl: &Egl, display: khronos_egl::Display) -> Result<()> {
    let key = display.as_ptr() as usize;
    let mut users = DISPLAY_USERS.lock().unwrap_or_else(|e| e.into_inner());
    match users.iter_mut().find(|(handle, _)| *handle == key) {
        Some((_, count)) => *count += 1,
        None => {
            // Initializing more than once is permitted and is what makes the
            // count meaningful, but doing it under the lock keeps a second
            // thread from terminating between the two.
            egl.initialize(display)
                .map_err(|e| backend_err("initialize", e))?;
            users.push((key, 1));
        }
    }
    Ok(())
}

/// Drop a user, terminating the display once the last one goes.
fn release_display(egl: &Egl, display: khronos_egl::Display) {
    let key = display.as_ptr() as usize;
    let mut users = DISPLAY_USERS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(index) = users.iter().position(|(handle, _)| *handle == key) {
        users[index].1 -= 1;
        if users[index].1 == 0 {
            users.swap_remove(index);
            let _ = egl.terminate(display);
        }
    }
}

impl std::fmt::Debug for GlesContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlesContext")
            .field("device", &self.capabilities.device_name)
            .field("driver", &self.capabilities.driver_name)
            .finish_non_exhaustive()
    }
}

fn backend_err(what: &str, e: khronos_egl::Error) -> Error {
    Error::Backend {
        backend: "gles",
        detail: format!("{what}: {e}"),
    }
}

fn split_extensions(list: &str) -> HashSet<String> {
    list.split_whitespace().map(|s| s.to_owned()).collect()
}

fn gl_extension_set(gl: &glow::Context) -> HashSet<String> {
    // SAFETY: a context is current, and the count is queried before indexing.
    unsafe {
        let count = gl.get_parameter_i32(glow::NUM_EXTENSIONS).max(0) as u32;
        (0..count)
            .map(|i| gl.get_parameter_indexed_string(glow::EXTENSIONS, i))
            .collect()
    }
}

fn detect_capabilities(gl: &glow::Context, egl_extensions: &HashSet<String>) -> Capabilities {
    // SAFETY: a context is current on this thread.
    let (max_texture_size, max_samples, renderer, version) = unsafe {
        (
            gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE).max(0) as u32,
            gl.get_parameter_i32(glow::MAX_SAMPLES).max(1) as u32,
            gl.get_parameter_string(glow::RENDERER),
            gl.get_parameter_string(glow::VERSION),
        )
    };

    // GLES reports only a maximum rather than a mask, so every power of two up
    // to it is assumed usable. That matches how the specification defines the
    // limit and how every driver behaves; a device that rejected an
    // intermediate count would be reporting its maximum wrongly.
    let mut mask = 1u32;
    let mut n = 2u32;
    while n <= max_samples && n <= 16 {
        mask |= n;
        n <<= 1;
    }

    // Import and export are separate extensions here, unlike Vulkan where one
    // handle type covers both. A driver offering only import still supports the
    // GBM path, where GBM allocates and GLES renders into what it made.
    let dma_buf = DmaBufSupport {
        import: egl_extensions.contains(ext::DMA_BUF_IMPORT),
        export: egl_extensions.contains(ext::DMA_BUF_EXPORT),
        modifiers: egl_extensions.contains(ext::DMA_BUF_MODIFIERS),
    };

    let fence = egl_extensions.contains(ext::NATIVE_FENCE_SYNC);
    Capabilities {
        // No advanced blending: the extension GLES exposes for it requires the
        // fragment shader to declare `blend_support_all_equations`, and the
        // shader translator this backend generates GLSL with cannot emit that
        // qualifier. Reporting true and hoping would produce plain source-over
        // silently. Lifting this needs a hand-written GLSL fragment stage plus
        // blend barriers between overlapping draws wherever the coherent
        // variant of the extension is missing.
        advanced_blend: false,
        max_texture_size,
        sample_counts: SampleCounts::from_mask(mask),
        dma_buf,
        sync: SyncSupport {
            export_sync_file: fence,
            import_sync_file: fence,
        },
        render_formats: Vec::new(),
        device_name: renderer,
        driver_name: version,
    }
}
