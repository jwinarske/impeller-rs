//! Look at a corpus scene in a window, live.
//!
//! # What this is for
//!
//! The suite says whether a scene matches another backend and a software
//! reference. It does not say whether the scene looks right, and several
//! mistakes this renderer has made were of exactly that kind: a gradient that
//! agreed across backends and was flat for the last forty percent of its
//! length, a frosted panel that rendered identically with the filter on and
//! off. Those are visible in a second and invisible to a comparison against a
//! second implementation making the same mistake.
//!
//! So: the same corpus, on screen, resizable, one key to the next scene. The
//! contact sheet already shows every scene at once; this shows one at a size
//! where you can see it, and re-renders it as the window changes.
//!
//! # Why the scene is drawn through a texture
//!
//! Corpus scenes are authored at a fixed size in absolute coordinates. Their
//! recordings are in clip space, so submitting one straight into a window-sized
//! image would stretch it to whatever shape the window is. Rendering the scene
//! into a texture of its own size and then drawing that texture centered and
//! scaled keeps its aspect, and costs one extra pass on a tool where that does
//! not matter.

use impeller_core::{Canvas, Color, Paint, Rect, TileMode};
use impeller_hal::{Extent2D, PixelFormat, TextureDescriptor};
use impeller_hal_vulkan::{DevicePreference, VulkanContext, VulkanHal, VulkanTexture};
use impeller_present::PresentTarget;
use impeller_present_vk::{PresentMode, SwapchainTarget};
use impeller_testkit::{corpus, record_scene, Scene};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// Square, because most corpus scenes are, so the first view is undistorted.
const INITIAL: u32 = 768;

fn main() {
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("run");
}

struct App {
    scenes: Vec<Scene>,
    current: usize,
    /// Everything that needs a window, created together when one arrives.
    ///
    /// winit hands the window over in `resumed` rather than at construction, so
    /// this cannot be built until then -- and on a platform that suspends, it
    /// can go away and come back.
    live: Option<Live>,
}

struct Live {
    window: Window,
    ctx: VulkanContext,
    target: SwapchainTarget,
    surface: ash::vk::SurfaceKHR,
    /// The scene, rendered at its own size, ready to be drawn into the window.
    scene_texture: Option<(VulkanTexture, Extent2D)>,
}

impl App {
    fn new() -> Self {
        let scenes = corpus();
        assert!(!scenes.is_empty(), "the corpus is empty");
        Self {
            scenes,
            current: 0,
            live: None,
        }
    }

    fn step(&mut self, forward: bool) {
        let n = self.scenes.len();
        self.current = if forward {
            (self.current + 1) % n
        } else {
            (self.current + n - 1) % n
        };
        if let Some(live) = &mut self.live {
            live.drop_scene_texture();
            live.window.request_redraw();
        }
        eprintln!(
            "[{}/{}] {}",
            self.current + 1,
            self.scenes.len(),
            self.scenes[self.current].name
        );
    }
}

impl Live {
    fn drop_scene_texture(&mut self) {
        if let Some((texture, _)) = self.scene_texture.take() {
            self.ctx.destroy_texture(texture);
        }
    }

    /// Render the scene once, into a texture of the scene's own size.
    ///
    /// Kept until the scene changes: a resize redraws the window from this
    /// rather than re-recording, which is both faster and the honest thing to
    /// show -- the scene is authored at one size and the window is showing it
    /// larger.
    fn ensure_scene_texture(&mut self, scene: &Scene) {
        if self.scene_texture.is_some() {
            return;
        }
        let extent = Extent2D::new(scene.size.width, scene.size.height);
        let recording = match record_scene(scene) {
            Ok(recording) => recording,
            Err(e) => {
                eprintln!("cannot record {}: {e}", scene.name);
                return;
            }
        };
        let mut texture = match self
            .ctx
            .create_texture(&TextureDescriptor::offscreen(extent, PixelFormat::Rgba8Unorm))
        {
            Ok(texture) => texture,
            Err(e) => {
                eprintln!("cannot allocate a target for {}: {e}", scene.name);
                return;
            }
        };
        match impeller_core::execute::<VulkanHal>(&mut self.ctx, &mut texture, &recording, &[]) {
            Ok(()) => self.scene_texture = Some((texture, extent)),
            Err(e) => {
                eprintln!("cannot render {}: {e}", scene.name);
                self.ctx.destroy_texture(texture);
            }
        }
    }

    /// Draw the scene texture into the window, centered and scaled to fit.
    fn draw(&mut self, scene: &Scene) {
        self.ensure_scene_texture(scene);
        let Some((_, scene_extent)) = &self.scene_texture else {
            return;
        };
        let window = self.target.extent();
        // The largest whole-pixel rectangle of the scene's shape that fits.
        let scale = (window.width as f32 / scene_extent.width as f32)
            .min(window.height as f32 / scene_extent.height as f32)
            .max(0.01);
        let (w, h) = (
            scene_extent.width as f32 * scale,
            scene_extent.height as f32 * scale,
        );
        let (x, y) = (
            (window.width as f32 - w) / 2.0,
            (window.height as f32 - h) / 2.0,
        );

        let mut canvas = Canvas::new(window);
        // A ground that is neither black nor white, so a scene of either is
        // visible against it and the letterboxing is obviously not the scene.
        canvas.clear(Color::srgb(0.12, 0.12, 0.14, 1.0));
        let rect = Rect::new(x, y, x + w, y + h);
        if let Err(e) = canvas.draw_rect(
            rect,
            &Paint::image(0, rect)
                .with_tile_mode(TileMode::Decal)
                .with_anti_alias(false),
        ) {
            eprintln!("cannot compose the frame: {e}");
            return;
        }
        let recording = canvas.finish();

        let Some((texture, _)) = &self.scene_texture else {
            return;
        };
        if let Err(e) = self.target.acquire(&mut self.ctx) {
            eprintln!("acquire: {e}");
            return;
        }
        // Reborrowed here rather than held across the acquire, which needs the
        // context mutably.
        let images: Vec<&VulkanTexture> = vec![texture];
        if let Err(e) = self
            .target
            .submit_recording(&mut self.ctx, &recording, &images)
        {
            eprintln!("submit: {e}");
            return;
        }
        if let Err(e) = self.target.present(&mut self.ctx) {
            eprintln!("present: {e}");
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.live.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("impeller playground")
            .with_inner_size(winit::dpi::LogicalSize::new(INITIAL, INITIAL));
        let window = event_loop.create_window(attributes).expect("window");

        let mut ctx = VulkanContext::new(DevicePreference::Auto).expect("vulkan context");
        // The instance already asks for the platform surface extensions; this
        // is the caller bringing a surface, which is the arrangement the
        // presentation crate is built around.
        let surface = unsafe {
            ash_window::create_surface(
                ctx.raw_entry(),
                ctx.raw_instance(),
                window.display_handle().expect("display handle").as_raw(),
                window.window_handle().expect("window handle").as_raw(),
                None,
            )
        }
        .expect("surface");

        let size = window.inner_size();
        let extent = Extent2D::new(size.width.max(1), size.height.max(1));
        let target = SwapchainTarget::new(&mut ctx, surface, extent, PresentMode::Fifo)
            .expect("swapchain");

        eprintln!(
            "{} — {} scenes. Right/Left or Space to step, Q to quit.",
            ctx.capabilities().device_name,
            self.scenes.len()
        );
        eprintln!("[1/{}] {}", self.scenes.len(), self.scenes[0].name);

        self.live = Some(Live {
            window,
            ctx,
            target,
            surface,
            scene_texture: None,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(live) = &mut self.live {
                    let extent = Extent2D::new(size.width.max(1), size.height.max(1));
                    if let Err(e) = live.target.reconfigure(&mut live.ctx, extent) {
                        eprintln!("reconfigure: {e}");
                    }
                    live.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                let scene = self.scenes[self.current].clone();
                if let Some(live) = &mut self.live {
                    live.draw(&scene);
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::ArrowRight) | Key::Named(NamedKey::Space) => {
                        self.step(true)
                    }
                    Key::Named(NamedKey::ArrowLeft) => self.step(false),
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Character(ref c) if c.as_str() == "q" => event_loop.exit(),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        // Torn down in the order the objects depend on each other: the scene
        // texture and the swapchain belong to the context, and the surface
        // outlives the swapchain built on it.
        if let Some(mut live) = self.live.take() {
            live.drop_scene_texture();
            let Live {
                ctx,
                target,
                surface,
                ..
            } = live;
            let mut ctx = ctx;
            target.destroy(&mut ctx);
            // SAFETY: the swapchain built on it has just been destroyed.
            unsafe { impeller_present_vk::destroy_surface(&ctx, surface) };
        }
    }
}
