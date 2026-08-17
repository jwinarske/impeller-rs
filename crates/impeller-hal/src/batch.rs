//! Draws accumulated for one submission.
//!
//! A batch is a declarative description of a scene: shared geometry plus a
//! list of draws over it. Backends are handed the whole thing rather than a
//! stream of recording calls, which lets each decide how to realize it — a
//! Vulkan backend binds pipelines only where they change, and a record-and-
//! replay backend can inspect the whole batch before touching any state.

use crate::{BlendMode, Error, Material, Result, Scissor};

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
    pub clip: Option<Scissor>,
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
    vertices: Vec<[f32; 2]>,
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
    /// Shared vertex buffer, in clip space.
    pub fn vertices(&self) -> &[[f32; 2]] {
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
