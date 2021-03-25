// Copyright 2020-2021 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <https://opensource.org/licenses/MIT>.

//! Core data types for graphics code to use.

use cgmath::{EuclideanSpace as _, Point3, Vector3};
use luminance::{Semantics, Vertex};
use luminance_front::context::GraphicsContext;
use luminance_front::tess::{Mode, Tess};
use luminance_front::Backend;
use std::convert::TryFrom as _;

use crate::math::{Face, FreeCoordinate, GridCoordinate, GridVector, Rgba};
use crate::space::PackedLight;
use crate::triangulator::{BlockVertex, Coloring, GfxVertex};

/// Defines vertex array structure for luminance framework.
/// Note that each "wrapper" names a new type generated by the `derive(Semantics)`.
#[derive(Copy, Clone, Debug, Semantics)]
#[rustfmt::skip]
pub enum VertexSemantics {
    // TODO: revisit compact representations
    /// Vertex position.
    #[sem(name = "a_position", repr = "[f32; 3]", wrapper = "VertexPosition")]
    Position,
    /// Vertex normal (should be length 1).
    #[sem(name = "a_normal", repr = "[f32; 3]", wrapper = "VertexNormal")]
    Normal,
    /// Packed format:
    /// * If `[3]` is in the range 0.0 to 1.0, then the attribute is a solid RGBA color.
    /// * If `[3]` is -1.0, then the first three components are 3D texture coordinates.
    #[sem(name = "a_color_or_texture", repr = "[f32; 4]", wrapper = "VertexColorOrTexture")]
    ColorOrTexture,
    /// Interpolated texture coordinates are clamped to be ≥ this value, to avoid bleeding.
    #[sem(name = "a_clamp_min", repr = "[f32; 3]", wrapper = "VertexClampLow")]
    ClampLow,
    /// Interpolated texture coordinates are clamped to be ≤ this value, to avoid bleeding.
    #[sem(name = "a_clamp_max", repr = "[f32; 3]", wrapper = "VertexClampHigh")]
    ClampHigh,
    /// Diffuse lighting intensity; typically the color or texture should be multiplied by this.
    // TODO: look into packed repr for lighting, or switching to a 3D texture
    #[sem(name = "a_lighting", repr = "[f32; 3]", wrapper = "VertexLighting")]
    Lighting,
}

/// Vertex type sent to shader for rendering blocks (and, for the moment, other geometry,
/// but its attributes are heavily focused on blocks).
/// See [`VertexSemantics`] for the meaning of the fields.
#[derive(Clone, Copy, Debug, PartialEq, Vertex)]
#[vertex(sem = "VertexSemantics")]
pub struct LumBlockVertex {
    position: VertexPosition,
    normal: VertexNormal,
    color_or_texture: VertexColorOrTexture,
    clamp_min: VertexClampLow,
    clamp_max: VertexClampHigh,
    lighting: VertexLighting,
}

impl LumBlockVertex {
    /// A vertex which will not be rendered.
    pub const DUMMY: Self = Self {
        position: VertexPosition::new([f32::INFINITY, f32::INFINITY, f32::INFINITY]),
        normal: VertexNormal::new([0., 0., 0.]),
        color_or_texture: VertexColorOrTexture::new([0., 0., 0., 0.]),
        clamp_min: VertexClampLow::new([0., 0., 0.]),
        clamp_max: VertexClampHigh::new([0., 0., 0.]),
        lighting: VertexLighting::new([0., 0., 0.]),
    };

    /// Constructor taking our natural types instead of luminance specialized types.
    #[inline]
    pub fn new_colored(
        position: Point3<FreeCoordinate>,
        normal: Vector3<FreeCoordinate>,
        color: Rgba,
    ) -> Self {
        Self {
            position: VertexPosition::new(position.cast::<f32>().unwrap().into()),
            normal: VertexNormal::new(normal.cast::<f32>().unwrap().into()),
            color_or_texture: VertexColorOrTexture::new(color.into()),
            clamp_min: VertexClampLow::new([0., 0., 0.]),
            clamp_max: VertexClampHigh::new([0., 0., 0.]),
            lighting: VertexLighting::new([1.0, 1.0, 1.0]),
        }
    }

    /// Make an axis-aligned rectangle. Convenience for debug code.
    /// Assumes `size` is positive.
    #[allow(dead_code)]
    pub fn rectangle(
        origin: Vector3<f32>,
        size: Vector3<f32>,
        tex_origin: Vector3<f32>,
        tex_size: Vector3<f32>,
    ) -> Box<[Self; 6]> {
        let normal: Vector3<f32> = Vector3::new(0.0, 0.0, 1.0);
        let dx = Vector3::new(size.x, 0.0, 0.0);
        let dy = Vector3::new(0.0, size.y, 0.0);
        let tdx = Vector3::new(tex_size.x, 0.0, 0.0);
        let tdy = Vector3::new(0.0, tex_size.y, 0.0);
        let v = |p: Vector3<f32>, t: Vector3<f32>| Self {
            position: VertexPosition::new(p.into()),
            normal: VertexNormal::new(normal.into()),
            color_or_texture: VertexColorOrTexture::new(t.extend(-1.0).into()),
            clamp_min: VertexClampLow::new([0., 0., 0.]),
            clamp_max: VertexClampHigh::new([0., 0., 0.]),
            lighting: VertexLighting::new([1.0, 1.0, 1.0]),
        };
        Box::new([
            v(origin, tex_origin),
            v(origin + dx, tex_origin + tdx),
            v(origin + dx + dy, tex_origin + tdx + tdy),
            v(origin + dx + dy, tex_origin + tdx + tdy),
            v(origin + dy, tex_origin + tdy),
            v(origin, tex_origin),
        ])
    }
}

impl From<BlockVertex> for LumBlockVertex {
    #[inline]
    fn from(vertex: BlockVertex) -> Self {
        let position = vertex.position.cast::<f32>().unwrap().to_vec();
        let normal = VertexNormal::new(vertex.face.normal_vector::<f32>().into());
        match vertex.coloring {
            Coloring::Solid(color) => {
                let mut color_attribute = VertexColorOrTexture::new(color.into());
                // Clamp out-of-range alpha values so they fit into the
                // VertexColorOrTexture protocol (not less than zero).
                color_attribute[3] = color_attribute[3].min(1.).max(0.);
                Self {
                    position: VertexPosition::new(position.into()),
                    normal,
                    color_or_texture: color_attribute,
                    clamp_min: VertexClampLow::new([0., 0., 0.]),
                    clamp_max: VertexClampHigh::new([0., 0., 0.]),
                    lighting: VertexLighting::new([0., 0., 0.]),
                }
            }
            Coloring::Texture {
                pos: tc,
                clamp_min,
                clamp_max,
            } => Self {
                position: VertexPosition::new(position.into()),
                normal,
                color_or_texture: VertexColorOrTexture::new([tc[0], tc[1], tc[2], -1.0]),
                clamp_min: VertexClampLow::new(clamp_min.into()),
                clamp_max: VertexClampHigh::new(clamp_max.into()),
                lighting: VertexLighting::new([0., 0., 0.]),
            },
        }
    }
}

impl GfxVertex for LumBlockVertex {
    type Coordinate = f32;

    #[inline]
    fn instantiate(&mut self, offset: Vector3<Self::Coordinate>, lighting: PackedLight) {
        self.position.repr[0] += offset.x;
        self.position.repr[1] += offset.y;
        self.position.repr[2] += offset.z;
        self.lighting = VertexLighting::new(lighting.value().into());
    }

    #[inline]
    fn position(&self) -> Point3<Self::Coordinate> {
        Point3::from(self.position.repr)
    }

    #[inline]
    fn face(&self) -> Face {
        let normal: GridVector = Vector3::from(self.normal.repr).map(|c| c as GridCoordinate);
        Face::try_from(normal).unwrap_or(Face::Within)
    }
}

/// Constructs a <code>[Tess]&lt;[LumBlockVertex]&gt;</code> that renders nothing but does
/// not provoke a runtime error.
pub fn empty_tess<C>(context: &mut C) -> Tess<LumBlockVertex>
where
    C: GraphicsContext<Backend = Backend>,
{
    context
        .new_tess()
        .set_vertices(vec![LumBlockVertex::DUMMY])
        .set_mode(Mode::Triangle)
        .build()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Face, Rgb};
    use cgmath::Vector3;

    #[test]
    fn vertex_dummy() {
        assert!(!LumBlockVertex::DUMMY.position.repr[0].is_finite());
    }

    #[test]
    fn vertex_new_colored() {
        let vertex = LumBlockVertex::new_colored(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(4.0, 5.0, 6.0),
            Rgba::new(7.0, 8.0, 9.0, 0.5),
        );
        assert_eq!(vertex.position.repr, [1.0, 2.0, 3.0]);
        assert_eq!(vertex.normal.repr, [4.0, 5.0, 6.0]);
        assert_eq!(vertex.color_or_texture.repr, [7.0, 8.0, 9.0, 0.5]);
        assert_eq!(vertex.lighting.repr, [1.0, 1.0, 1.0]);
    }

    /// Full path used by normal rendering.
    #[test]
    fn vertex_from_block_vertex() {
        let block_vertex = BlockVertex {
            position: Point3::new(1.0, 2.0, 3.0),
            face: Face::PX,
            coloring: Coloring::Solid(Rgba::new(7.0, 8.0, 9.0, 0.5)),
        };
        let mut vertex = LumBlockVertex::from(block_vertex);
        vertex.instantiate(Vector3::new(0.1, 0.2, 0.3), Rgb::new(1.0, 0.0, 2.0).into());
        assert_eq!(vertex.position.repr, [1.1, 2.2, 3.3]);
        assert_eq!(vertex.normal.repr, [1.0, 0.0, 0.0]);
        assert_eq!(vertex.color_or_texture.repr, [7.0, 8.0, 9.0, 0.5]);
        assert_eq!(vertex.lighting.repr, [1.0, 0.0, 2.0]);
    }
}
