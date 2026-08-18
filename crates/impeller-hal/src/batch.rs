//! Draws accumulated for one submission.
//!
//! A batch is a declarative description of a scene: shared geometry plus a
//! list of draws over it. Backends are handed the whole thing rather than a
//! stream of recording calls, which lets each decide how to realize it — a
//! Vulkan backend binds pipelines only where they change, and a record-and-
//! replay backend can inspect the whole batch before touching any state.

use crate::{BlendMode, Error, Material, Result, Scissor};

/// What a draw does with the stencil buffer.
///
/// # Why the stencil holds a depth rather than a mask
///
/// The obvious encoding gives each clip a bit, which caps nesting at eight and
/// makes intersecting two clips a per-bit affair. Storing the *nesting depth*
/// instead lets a clip stack of any size fit in the same eight bits, and makes
/// the test a single comparison: content belongs to depth `d` and draws where
/// the stencil holds `d`, which is true only where every clip down to that
/// depth admitted the pixel.
///
/// It also makes undoing a clip a local operation. Because a stack unwinds in
/// the order it was built, no pixel can hold more than the depth being left, so
/// stepping back is a decrement rather than a recomputation from the remaining
/// clips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ClipRole {
    /// Draw color where the stencil already matches. Leaves the stencil alone.
    #[default]
    Content,
    /// Narrow the clip: step the stencil forward where it matches and this draw
    /// covers. Writes no color.
    ///
    /// The geometry must be a triangulation of the clip region rather than an
    /// overlapping set, since a pixel covered twice would step forward twice
    /// and stop matching anything. The fill tessellator produces exactly that,
    /// which is what lets this be a plain increment instead of the parity trick
    /// an overlapping fan would need.
    Narrow,
    /// Widen the clip back: step the stencil back where it matches. Writes no
    /// color.
    Widen,
}

impl ClipRole {
    /// Whether this role writes to the color attachment.
    pub const fn writes_color(self) -> bool {
        matches!(self, Self::Content)
    }

    /// Whether this role modifies the stencil.
    pub const fn writes_stencil(self) -> bool {
        !matches!(self, Self::Content)
    }
}

/// The stencil state one draw needs.
///
/// `reference` is what the stencil is compared against, stated directly rather
/// than derived from a nesting depth, so the HAL needs no notion of a clip
/// stack: a narrowing draw compares against the depth it is leaving and a
/// widening draw against the one it is leaving behind, and which is which is
/// the recorder's business.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ClipState {
    pub reference: u32,
    pub role: ClipRole,
}

impl ClipState {
    /// Content outside any clip, which needs no stencil at all.
    pub const UNCLIPPED: Self = Self {
        reference: 0,
        role: ClipRole::Content,
    };

    pub const fn content(reference: u32) -> Self {
        Self {
            reference,
            role: ClipRole::Content,
        }
    }

    pub const fn narrow(from: u32) -> Self {
        Self {
            reference: from,
            role: ClipRole::Narrow,
        }
    }

    pub const fn widen(from: u32) -> Self {
        Self {
            reference: from,
            role: ClipRole::Widen,
        }
    }

    /// Whether this needs a stencil attachment to mean anything.
    pub const fn needs_stencil(self) -> bool {
        self.reference != 0 || self.role.writes_stencil()
    }
}

/// One vertex: where it is, and where it reads from.
///
/// # Why every vertex carries texture coordinates
///
/// Most geometry here does not need them — a solid fill and a gradient both
/// locate themselves from the interpolated clip position. A glyph run does: a
/// run is many quads reading different parts of one atlas, and a material is
/// per draw, so coordinates carried in the paint would mean a draw per glyph.
/// Text is the highest draw-count content there is, so that is the wrong place
/// to spend.
///
/// The cost is eight bytes on every vertex, including the ones that ignore
/// them. The alternative — a second vertex format and a second pipeline for
/// text — spends more in pipeline state and in the code that has to decide
/// which of two shapes a batch is in, to save memory on the geometry that is
/// already the cheapest to store.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[repr(C)]
pub struct Vertex {
    pub position: [f32; 2],
    /// Where in a sampled texture this vertex reads, if the material samples
    /// one. Zero where it does not, which costs nothing to interpolate.
    pub uv: [f32; 2],
}

impl Vertex {
    pub const fn new(position: [f32; 2], uv: [f32; 2]) -> Self {
        Self { position, uv }
    }

    /// A vertex that samples nothing.
    pub const fn at(position: [f32; 2]) -> Self {
        Self::new(position, [0.0, 0.0])
    }
}

/// One draw within a batch.
#[derive(Debug, Clone)]
pub struct BatchDraw {
    pub first_index: u32,
    pub index_count: u32,
    pub material: Material,
    pub blend: BlendMode,
    /// The region of the target this draw may write to.
    ///
    /// `None` is the whole target. It is distinct from a rectangle that happens
    /// to cover the target so a backend can tell "this draw was never clipped"
    /// from "this draw's clip works out to everything", and skip the state
    /// change in the first case without having to know the target's size.
    ///
    /// Independent of [`Self::stencil`], and both apply. An axis-aligned clip
    /// stays here even where a stencil is already in play, because a scissor is
    /// exact and costs nothing while a stencil pass costs a draw.
    pub clip: Option<Scissor>,
    /// What this draw does with the stencil buffer.
    pub stencil: ClipState,
}

/// Geometry and paint for a sequence of draws sharing one target.
///
/// Draws are kept in submission order rather than sorted by pipeline. Sorting
/// would cut pipeline binds, but 2D drawing is painter's-algorithm ordered:
/// reordering two overlapping draws changes which one ends up on top. Deciding
/// when a reorder is safe needs either overlap analysis or a depth buffer, and
/// that belongs to the layer that knows what the draws represent.
#[derive(Debug, Default, Clone)]
pub struct Batch {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    draws: Vec<BatchDraw>,
}

impl Batch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a draw covering the whole target.
    ///
    /// Indices are relative to `vertices` and are rebased onto the batch's
    /// shared buffer, so a caller need not know what came before it.
    pub fn push(
        &mut self,
        vertices: &[[f32; 2]],
        indices: &[u32],
        material: Material,
        blend: BlendMode,
    ) -> Result<()> {
        self.push_clipped(vertices, indices, material, blend, None)
    }

    /// Append a draw confined to a region of the target.
    ///
    /// A separate entry point rather than an extra parameter on [`Self::push`]:
    /// most draws are unclipped, and threading `None` through every call site
    /// makes the ones that do carry a clip harder to pick out, not easier.
    ///
    /// An empty scissor drops the draw. Recording something that provably
    /// writes no pixel would cost a pipeline bind and a draw call to produce
    /// the same target, and a clip stack that has narrowed to nothing is a
    /// normal state for a scrolled-away subtree rather than an error.
    pub fn push_clipped(
        &mut self,
        vertices: &[[f32; 2]],
        indices: &[u32],
        material: Material,
        blend: BlendMode,
        clip: Option<Scissor>,
    ) -> Result<()> {
        self.push_with(
            vertices,
            indices,
            material,
            blend,
            clip,
            ClipState::UNCLIPPED,
        )
    }

    /// Append a draw with an explicit stencil role.
    ///
    /// The general form the other two delegate to. A caller reaches for this
    /// only when building or unwinding a clip, or when drawing content inside
    /// one; everything else is confined by a scissor or not confined at all.
    pub fn push_with(
        &mut self,
        positions: &[[f32; 2]],
        indices: &[u32],
        material: Material,
        blend: BlendMode,
        clip: Option<Scissor>,
        stencil: ClipState,
    ) -> Result<()> {
        // Tessellated geometry has no texture coordinates of its own, and the
        // materials it carries do not read them.
        let vertices: Vec<Vertex> = positions.iter().copied().map(Vertex::at).collect();
        self.push_mesh(&vertices, indices, material, blend, clip, stencil)
    }

    /// Append a draw whose vertices carry texture coordinates.
    ///
    /// The form a glyph run takes: one draw over many quads, each reading a
    /// different part of the same atlas.
    #[allow(clippy::too_many_arguments)]
    pub fn push_mesh(
        &mut self,
        vertices: &[Vertex],
        indices: &[u32],
        material: Material,
        blend: BlendMode,
        clip: Option<Scissor>,
        stencil: ClipState,
    ) -> Result<()> {
        if clip.is_some_and(Scissor::is_empty) {
            return Ok(());
        }
        if indices.len() % 3 != 0 {
            return Err(Error::Unsupported("index count is not a whole triangle"));
        }
        if let Some(&max) = indices.iter().max() {
            if max as usize >= vertices.len() {
                return Err(Error::Backend {
                    backend: "vulkan",
                    detail: format!(
                        "index {max} addresses past the {} vertices supplied",
                        vertices.len()
                    ),
                });
            }
        }
        if indices.is_empty() {
            return Ok(());
        }

        let base = u32::try_from(self.vertices.len()).map_err(|_| Error::LimitExceeded {
            what: "batch vertex count",
            requested: self.vertices.len() as u64,
            limit: u32::MAX as u64,
        })?;
        let first_index = self.indices.len() as u32;

        self.vertices.extend_from_slice(vertices);
        self.indices.extend(indices.iter().map(|i| i + base));
        self.draws.push(BatchDraw {
            first_index,
            index_count: indices.len() as u32,
            material,
            blend,
            clip,
            stencil,
        });
        Ok(())
    }

    /// Drop the contents but keep the allocations, for reuse next frame.
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.draws.clear();
    }

    pub fn draw_count(&self) -> usize {
        self.draws.len()
    }

    pub fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }

    /// Whether recording this needs a stencil attachment.
    ///
    /// Derived from the draws rather than declared alongside them, so a batch
    /// cannot ask for a clip and forget to say it needs somewhere to put it.
    /// Most batches clip nothing, and those pay for no attachment.
    pub fn uses_stencil(&self) -> bool {
        self.draws.iter().any(|draw| draw.stencil.needs_stencil())
    }

    /// The deepest clip stack an eight-bit stencil can distinguish.
    ///
    /// Eight bits is the only stencil depth every device is required to offer,
    /// on either graphics API, so this is the portable limit rather than any
    /// one device's.
    pub const MAX_CLIP_DEPTH: u32 = 255;

    /// Refuse a batch whose clip stack is deeper than a stencil can hold.
    ///
    /// Here rather than in each backend because the limit is a property of the
    /// stencil format both are required to offer, and the failure it prevents
    /// is one neither can detect afterwards: past the limit the value wraps or
    /// saturates, and either way a later test for a depth that no longer fits
    /// admits every pixel the clip was meant to exclude. Nothing about that
    /// looks like an error -- it draws content the caller clipped away.
    ///
    /// It was in one backend and not the other, so the same recording was
    /// refused on Vulkan and silently rendered wrong on GLES.
    pub fn check_clip_depth(&self) -> Result<()> {
        let depth = self.max_clip_depth();
        if depth > Self::MAX_CLIP_DEPTH {
            return Err(Error::LimitExceeded {
                what: "clip nesting depth",
                requested: depth as u64,
                limit: Self::MAX_CLIP_DEPTH as u64,
            });
        }
        Ok(())
    }

    /// The largest stencil value this batch can produce.
    pub fn max_clip_depth(&self) -> u32 {
        self.draws
            .iter()
            .map(|draw| match draw.stencil.role {
                ClipRole::Narrow => draw.stencil.reference + 1,
                _ => draw.stencil.reference,
            })
            .max()
            .unwrap_or(0)
    }

    /// The texture slots this batch samples, in ascending order without
    /// repeats.
    ///
    /// A backend uses this to size its bindings before recording, and to check
    /// the table it was given covers what the draws ask for.
    pub fn texture_slots(&self) -> Vec<u32> {
        let mut slots: Vec<u32> = self
            .draws
            .iter()
            .filter_map(|draw| draw.material.texture_slot())
            .collect();
        slots.sort_unstable();
        slots.dedup();
        slots
    }

    /// How many times a pipeline will be bound when this batch is recorded.
    ///
    /// Consecutive draws sharing a blend mode reuse the bound pipeline, so this
    /// counts transitions rather than draws.
    pub fn pipeline_binds(&self) -> usize {
        let mut binds = 0;
        let mut current: Option<BlendMode> = None;
        for draw in &self.draws {
            if current != Some(draw.blend) {
                binds += 1;
                current = Some(draw.blend);
            }
        }
        binds
    }
}

impl Batch {
    /// Shared vertex buffer, positions in clip space.
    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }

    /// Shared index buffer, already rebased onto [`Batch::vertices`].
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// The draws, in submission order.
    pub fn draws(&self) -> &[BatchDraw] {
        &self.draws
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRI: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];

    #[test]
    fn indices_are_rebased_onto_the_shared_buffer() {
        let mut batch = Batch::new();
        batch
            .push(&TRI, &[0, 1, 2], Material::solid([1.0; 4]), BlendMode::Src)
            .unwrap();
        batch
            .push(&TRI, &[0, 1, 2], Material::solid([1.0; 4]), BlendMode::Src)
            .unwrap();

        // The second draw's indices must point at its own vertices, not the
        // first draw's, or both draws render the same triangle.
        assert_eq!(batch.indices, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(batch.vertices.len(), 6);
        assert_eq!(batch.draws[1].first_index, 3);
        assert_eq!(batch.draw_count(), 2);
    }

    #[test]
    fn pipeline_binds_count_transitions_not_draws() {
        let mut batch = Batch::new();
        for blend in [
            BlendMode::Src,
            BlendMode::Src,
            BlendMode::SrcOver,
            BlendMode::SrcOver,
            BlendMode::Src,
        ] {
            batch
                .push(&TRI, &[0, 1, 2], Material::solid([1.0; 4]), blend)
                .unwrap();
        }
        // Five draws, three runs of like pipelines.
        assert_eq!(batch.draw_count(), 5);
        assert_eq!(batch.pipeline_binds(), 3);
    }

    #[test]
    fn an_empty_draw_adds_nothing() {
        let mut batch = Batch::new();
        batch
            .push(&[], &[], Material::solid([1.0; 4]), BlendMode::Src)
            .unwrap();
        assert!(batch.is_empty());
        assert_eq!(batch.draw_count(), 0);
    }

    #[test]
    fn malformed_geometry_is_refused_where_it_is_pushed() {
        let mut batch = Batch::new();
        // Catching this at push means the caller learns which draw was wrong,
        // rather than a whole batch failing later at submission.
        assert!(batch
            .push(
                &[[0.0, 0.0]],
                &[0, 1, 2],
                Material::solid([1.0; 4]),
                BlendMode::Src
            )
            .is_err());
        assert!(batch
            .push(
                &[[0.0, 0.0]],
                &[0, 0],
                Material::solid([1.0; 4]),
                BlendMode::Src
            )
            .is_err());
        assert!(batch.is_empty(), "a refused draw must leave no residue");
    }

    #[test]
    fn clearing_keeps_the_batch_reusable() {
        let mut batch = Batch::new();
        batch
            .push(&TRI, &[0, 1, 2], Material::solid([1.0; 4]), BlendMode::Src)
            .unwrap();
        batch.clear();
        assert!(batch.is_empty());

        batch
            .push(&TRI, &[0, 1, 2], Material::solid([1.0; 4]), BlendMode::Src)
            .unwrap();
        // Rebasing must start from zero again rather than continuing from the
        // cleared contents.
        assert_eq!(batch.indices, vec![0, 1, 2]);
    }
}
