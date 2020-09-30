// Copyright 2020 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <http://opensource.org/licenses/MIT>.

//! OpenGL-based graphics rendering.

use cgmath::{Matrix4, Point2, SquareMatrix as _, Vector3, Zero as _};
use luminance_front::context::GraphicsContext;
use luminance_front::face_culling::{FaceCulling, FaceCullingMode, FaceCullingOrder};
use luminance_front::framebuffer::Framebuffer;
use luminance_front::pipeline::PipelineState;
use luminance_front::render_state::RenderState;
use luminance_front::tess::{Mode, Tess};
use luminance_front::texture::Dim2;
use luminance_front::Backend;

use all_is_cubes::camera::{cursor_raycast, Camera, Cursor, ProjectionHelper};
use all_is_cubes::lum::shading::{BlockProgram, prepare_block_program};
use all_is_cubes::lum::space::{SpaceRenderInfo, SpaceRenderer};
use all_is_cubes::lum::types::Vertex;
use all_is_cubes::math::{GridCoordinate, RGBA};
use all_is_cubes::universe::URef;
use all_is_cubes::util::WarningsResult;

use crate::js_bindings::CanvasHelper;

pub struct GLRenderer<C>
where
    C: GraphicsContext<Backend = Backend>,
{
    // Graphics objects
    surface: C,
    back_buffer: Framebuffer<Dim2, (), ()>,
    block_program: BlockProgram,

    // Rendering state
    camera: Option<URef<Camera>>,
    space_renderer: Option<SpaceRenderer>,

    // Miscellaneous
    canvas_helper: CanvasHelper,
    proj: ProjectionHelper,
    pub(crate) cursor_result: Option<Cursor>,
}

impl<C> GLRenderer<C>
where
    C: GraphicsContext<Backend = Backend>,
{
    pub fn new(
        mut surface: C,
        canvas_helper: CanvasHelper,
    ) -> WarningsResult<Self, String, String> {
        // TODO: If WarningsResult continues being a thing, need a better success propagation strategy
        let (block_program, warnings) = prepare_block_program(&mut surface)?;

        let proj = ProjectionHelper::new(1.0, canvas_helper.viewport_px());
        let back_buffer = luminance::framebuffer::Framebuffer::back_buffer(
            &mut surface,
            canvas_helper.viewport_dev(),
        )
        .unwrap(); // TODO error handling
        Ok((
            Self {
                surface,
                back_buffer,
                block_program,
                camera: None,
                space_renderer: None,
                canvas_helper,
                proj,
                cursor_result: None,
            },
            warnings,
        ))
    }

    pub fn update_viewport(&mut self) {
        self.proj.set_viewport(self.canvas_helper.viewport_px());
        self.back_buffer = luminance::framebuffer::Framebuffer::back_buffer(
            &mut self.surface,
            self.canvas_helper.viewport_dev(),
        )
        .unwrap(); // TODO error handling
    }

    pub fn set_camera(&mut self, camera: Option<URef<Camera>>) {
        self.camera = camera;
    }

    pub fn render_frame(&mut self) -> RenderInfo {
        let mut info = RenderInfo::default();
        let camera: &Camera = &*(if let Some(camera_ref) = &self.camera {
            camera_ref.borrow()
        } else {
            return info;
        });
        let surface = &mut self.surface;
        let block_program = &mut self.block_program;
        let projection_matrix = self.proj.projection();

        // Update cursor state. This is, strictly speaking, not rendering, but it is closely
        // related in that the cursor should match the pixels being drawn.
        // TODO: Figure out how to lay this out with more separation of concerns, though.
        self.proj.set_view_matrix(camera.view());
        self.cursor_result = cursor_raycast(
            self.proj.project_cursor_into_world().cast(),
            &*camera.space.borrow(),
        );

        if self.space_renderer.as_ref().map(|sr| sr.space()) != Some(&camera.space) {
            self.space_renderer = Some(SpaceRenderer::new(camera.space.clone()));
        }
        let space_renderer = self.space_renderer.as_mut().unwrap();
        let space_output = space_renderer.prepare_frame(surface, camera.view());

        let render_state = RenderState::default().set_face_culling(FaceCulling {
            order: FaceCullingOrder::CCW,
            mode: FaceCullingMode::Back,
        });

        // let debug_tess = block_data.texture_allocator.debug_atlas_tess(surface);

        // TODO: cache
        let cursor_tess = make_cursor_tess(surface, &self.cursor_result);

        let render = surface
            .new_pipeline_gate()
            .pipeline(
                &self.back_buffer,
                // TODO: port skybox cube map code
                &PipelineState::default().set_clear_color([0.6, 0.7, 1.0, 1.]),
                |pipeline, mut shading_gate| {
                    let space_output_bound = space_output.bind(&pipeline)?;

                    shading_gate.shade(block_program, |mut program_iface, u, mut render_gate| {
                        u.set_projection_matrix(&mut program_iface, projection_matrix);

                        // Render space (and cursor).
                        u.set_view_matrix(&mut program_iface, space_output_bound.view_matrix);
                        u.set_block_texture(
                            &mut program_iface,
                            &space_output_bound.bound_block_texture,
                        );
                        render_gate.render(&render_state, |mut tess_gate| {
                            // TODO: should be `info.space += ...`
                            info.space = space_output_bound.render(&mut tess_gate)?;
                            tess_gate.render(&cursor_tess)?;
                            Ok(())
                        })?;

                        if false {
                            // Reset matrices for screen-aligned debug rendering
                            u.set_projection_matrix(&mut program_iface, Matrix4::identity());
                            u.set_view_matrix(&mut program_iface, Matrix4::identity());
                            // already set block texture

                            // render_gate.render(&render_state, |mut tess_gate| {
                            //     tess_gate.render(&debug_tess)?;
                            //     Ok(())
                            // })?;
                        }

                        Ok(())
                    })
                },
            )
            .assume(); // TODO error handling

        if !render.is_ok() {
            panic!("not ok"); // TODO what good error handling goes here?
        }

        // There is no swap_buffers operation because WebGL implicitly does so.
        info
    }

    // TODO: This is a workaround for self.proj being private; arguably doesn't even belong there or here. Find a better structure.
    pub fn set_cursor_position(&mut self, position: Point2<usize>) {
        self.proj.set_cursor_position(position);
    }
}

/// Information about render performance
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderInfo {
    space: SpaceRenderInfo,
}

fn make_cursor_tess<C>(context: &mut C, cursor_result: &Option<Cursor>) -> Tess<Vertex>
where
    C: GraphicsContext<Backend = Backend>,
{
    // TODO: reuse instead of building anew
    let mut vertices = Vec::with_capacity(3 /* axes */ * 4 /* lines */ * 2 /* vertices */);
    if let Some(cursor) = cursor_result {
        let origin = cursor.place.cube;
        let cursor_vertex = |v: Vector3<GridCoordinate>| {
            Vertex::new_colored(
                (origin + v).cast::<f64>().unwrap(),
                Vector3::zero(),
                RGBA::BLACK,
            )
        };
        for axis in 0..3 {
            let mut offset = Vector3::zero();
            // Walk from (0, 0, 0) to (1, 1, 1) in a helix.
            vertices.push(cursor_vertex(offset));
            offset[axis] = 1;
            vertices.push(cursor_vertex(offset));
            vertices.push(cursor_vertex(offset));
            offset[(axis + 1).rem_euclid(3)] = 1;
            vertices.push(cursor_vertex(offset));
            vertices.push(cursor_vertex(offset));
            offset[(axis + 2).rem_euclid(3)] = 1;
            vertices.push(cursor_vertex(offset));
            // Go back and fill in the remaining bar.
            offset[(axis + 2).rem_euclid(3)] = 0;
            vertices.push(cursor_vertex(offset));
            offset[axis] = 0;
            vertices.push(cursor_vertex(offset));
        }
    } else {
        vertices.push(Vertex::DUMMY);
    };
    context
        .new_tess()
        .set_vertices(vertices)
        .set_mode(Mode::Line)
        .build()
        .unwrap()
}
