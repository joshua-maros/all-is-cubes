// Copyright 2020-2021 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <https://opensource.org/licenses/MIT>.

//! Raytracer for [`Space`]s.
//!
//! ## Why?
//!
//! The original reason this exists is that I thought “we have `all_is_cubes::raycast`,
//! and that's nearly all the work, so why not?” Secondarily, it was written before
//! the mesh-based renderer `all_is_cubes::lum`, and was useful as a cross-check since
//! it is much simpler.
//!
//! In the future (or currently, if I forgot to update this comment), it will be used
//! as a means to display the state of `Space`s used for testing inline in test output.

use cgmath::{EuclideanSpace as _, InnerSpace as _, Matrix4, Point2, Vector2, Vector3, Zero as _};
use cgmath::{Point3, Vector4};
use ouroboros::self_referencing;
#[cfg(feature = "rayon")]
use rayon::iter::{IntoParallelIterator as _, ParallelIterator as _};
use std::borrow::Cow;
use std::convert::TryFrom;

use crate::block::{recursive_ray, Evoxel, Resolution};
use crate::camera::{eye_for_look_at, Camera, GraphicsOptions, LightingOption, Viewport};
use crate::math::{smoothstep, GridCoordinate};
use crate::math::{Face, FreeCoordinate, GridPoint, Rgb, Rgba};
use crate::raycast::Ray;
use crate::space::{Grid, GridArray, PackedLight, Space, SpaceBlockData};

/// Precomputed data for raytracing a single frame of a single Space, and bearer of the
/// methods for actually performing raytracing.
pub struct SpaceRaytracer<P: PixelBuf>(SpaceRaytracerImpl<P>);

/// Helper struct for [`SpaceRaytracer`] so the details of [`ouroboros::self_referencing`]
/// aren't exposed.
#[self_referencing]
struct SpaceRaytracerImpl<P: PixelBuf> {
    blocks: Box<[TracingBlock<P::BlockData>]>,
    #[borrows(blocks)]
    #[covariant]
    cubes: GridArray<TracingCubeData<'this, P::BlockData>>,

    options: GraphicsOptions,
    sky_color: Rgb,
}

impl<P: PixelBuf> SpaceRaytracer<P> {
    /// Snapshots the given [`Space`] to prepare for raytracing it.
    pub fn new(space: &Space, options: GraphicsOptions) -> Self {
        SpaceRaytracer(
            SpaceRaytracerImplBuilder {
                blocks: prepare_blocks::<P>(space),
                cubes_builder: |blocks: &Box<[TracingBlock<P::BlockData>]>| {
                    prepare_cubes::<P>(blocks, space)
                },
                options,
                sky_color: space.physics().sky_color,
            }
            .build(),
        )
    }

    /// Computes a single image pixel from the given ray.
    pub fn trace_ray(&self, ray: Ray) -> (P::Pixel, RaytraceInfo) {
        self.0.with(|impl_fields| {
            let cubes = impl_fields.cubes;
            let mut s: TracingState<P> = TracingState::default();
            for hit in ray.cast().within_grid(cubes.grid()) {
                if s.count_step_should_stop() {
                    break;
                }

                match &cubes[hit.cube_ahead()].block {
                    TracingBlock::Atom(pixel_block_data, color) => {
                        if color.fully_transparent() {
                            continue;
                        }
                        // TODO: To implement TransparencyOption::Volumetric we need to peek forward to the next change of color and find the distance between them, but only if the alpha is not 0 or 1. (Same here and in the recursive block case.)
                        s.trace_through_surface(
                            pixel_block_data,
                            *color,
                            match impl_fields.options.lighting_display {
                                LightingOption::None => Rgb::ONE,
                                LightingOption::Flat => self.get_lighting(hit.cube_behind()),
                                LightingOption::Smooth => self.get_interpolated_light(
                                    hit.intersection_point(ray),
                                    hit.face(),
                                ),
                            },
                            hit.face(),
                            &impl_fields.options,
                        );
                    }
                    TracingBlock::Recur(pixel_block_data, resolution, array) => {
                        let resolution = *resolution;
                        let sub_ray = recursive_ray(ray, hit.cube_ahead(), resolution);
                        let antiscale = FreeCoordinate::from(resolution).recip();
                        for subcube_hit in sub_ray.cast().within_grid(Grid::for_block(resolution)) {
                            if s.count_step_should_stop() {
                                break;
                            }
                            if let Some(voxel) = array.get(subcube_hit.cube_ahead()) {
                                s.trace_through_surface(
                                    pixel_block_data,
                                    voxel.color,
                                    match impl_fields.options.lighting_display {
                                        LightingOption::None => Rgb::ONE,
                                        LightingOption::Flat => self.get_lighting(
                                            hit.cube_ahead() + subcube_hit.face().normal_vector(),
                                        ),
                                        LightingOption::Smooth => self.get_interpolated_light(
                                            subcube_hit.intersection_point(sub_ray) * antiscale
                                                + hit
                                                    .cube_ahead()
                                                    .map(FreeCoordinate::from)
                                                    .to_vec(),
                                            subcube_hit.face(),
                                        ),
                                    },
                                    subcube_hit.face(),
                                    &impl_fields.options,
                                );
                            }
                        }
                    }
                }
            }
            s.finish(*impl_fields.sky_color)
        })
    }

    /// Compute a full image.
    ///
    /// The returned `[P::Pixel]` is in the usual left-right then top-bottom raster order;
    /// its dimensions are `camera.framebuffer_size`.
    ///
    /// TODO: Add a mechanism for incrementally rendering into a mutable buffer instead of
    /// all-at-once into a newly allocated one, for interactive use.
    pub fn trace_scene_to_image(&self, camera: &Camera) -> (Box<[P::Pixel]>, RaytraceInfo) {
        // This wrapper function ensures that the two implementations have consistent
        // signatures.
        self.trace_scene_to_image_impl(camera)
    }

    #[cfg(feature = "rayon")]
    fn trace_scene_to_image_impl(&self, camera: &Camera) -> (Box<[P::Pixel]>, RaytraceInfo) {
        let viewport = camera.viewport();
        let viewport_size = viewport.framebuffer_size.map(|s| s as usize);

        let output_iterator = (0..viewport_size.y)
            .into_par_iter()
            .map(move |ych| {
                let y = viewport.normalize_fb_y(ych);
                (0..viewport_size.x).into_par_iter().map(move |xch| {
                    let x = viewport.normalize_fb_x(xch);
                    self.trace_ray(camera.project_ndc_into_world(Point2::new(x, y)))
                })
            })
            .flatten();

        let (image, info_sum): (Vec<P::Pixel>, rayon_helper::ParExtSum<RaytraceInfo>) =
            output_iterator.unzip();

        (image.into_boxed_slice(), info_sum.result())
    }

    #[cfg(not(feature = "rayon"))]
    fn trace_scene_to_image_impl(&self, camera: &Camera) -> (Box<[P::Pixel]>, RaytraceInfo) {
        let viewport = camera.viewport();
        let viewport_size = viewport.framebuffer_size.map(|s| s as usize);
        let mut image = Vec::with_capacity(viewport.pixel_count().expect("image too large"));

        let mut total_info = RaytraceInfo::default();
        for ych in 0..viewport_size.y {
            let y = viewport.normalize_fb_y(ych);
            for xch in 0..viewport_size.x {
                let x = viewport.normalize_fb_x(xch);
                let (pixel, info) =
                    self.trace_ray(camera.project_ndc_into_world(Point2::new(x, y)));
                total_info += info;
                image.push(pixel);
            }
        }

        (image.into_boxed_slice(), total_info)
    }

    #[inline]
    fn get_packed_light(&self, cube: GridPoint) -> PackedLight {
        // TODO: wrong unwrap_or value
        self.0.with(|impl_fields| {
            impl_fields
                .cubes
                .get(cube)
                .map(|b| b.lighting)
                .unwrap_or(PackedLight::NO_RAYS)
        })
    }

    #[inline]
    fn get_lighting(&self, cube: GridPoint) -> Rgb {
        self.0.with(|impl_fields| {
            impl_fields
                .cubes
                .get(cube)
                .map(|b| b.lighting.value())
                .unwrap_or(*impl_fields.sky_color)
        })
    }

    fn get_interpolated_light(&self, point: Point3<FreeCoordinate>, face: Face) -> Rgb {
        // This implementation is duplicated in GLSL at src/lum/shaders/fragment.glsl

        // About half the size of the smallest permissible voxel.
        let above_surface_epsilon = 0.5 / 256.0;

        // The position we should start with for light lookup and interpolation.
        let origin = point.to_vec() + face.normal_vector() * above_surface_epsilon;

        // Find linear interpolation coefficients based on where we are relative to
        // a half-cube-offset grid.
        let reference_frame = face.matrix(0).to_free();
        let mut mix_1 = (origin.dot(reference_frame.x.truncate()) - 0.5).rem_euclid(1.0);
        let mut mix_2 = (origin.dot(reference_frame.y.truncate()) - 0.5).rem_euclid(1.0);

        // Ensure that mix <= 0.5, i.e. the 'near' side below is the side we are on
        fn flip_mix(
            mix: &mut FreeCoordinate,
            dir: Vector4<FreeCoordinate>,
        ) -> Vector3<FreeCoordinate> {
            let dir = dir.truncate();
            if *mix > 0.5 {
                *mix = 1.0 - *mix;
                -dir
            } else {
                dir
            }
        }
        let dir_1 = flip_mix(&mut mix_1, reference_frame.x);
        let dir_2 = flip_mix(&mut mix_2, reference_frame.y);

        // Modify interpolation by smoothstep to change the visual impression towards
        // "blurred blocks" and away from the diamond-shaped gradients of linear interpolation
        // which, being so familiar, can give an unfortunate impression of "here is
        // a closeup of a really low-resolution texture".
        let mix_1 = smoothstep(mix_1);
        let mix_2 = smoothstep(mix_2);

        // Retrieve light data, again using the half-cube-offset grid (this way we won't have edge artifacts).
        let get_light = |p: Vector3<FreeCoordinate>| {
            self.get_packed_light(Point3::from_vec(
                (origin + p).map(|s| s.floor() as GridCoordinate),
            ))
        };
        let lin_lo = -0.5;
        let lin_hi = 0.5;
        let near12 = get_light(lin_lo * dir_1 + lin_lo * dir_2);
        let near1far2 = get_light(lin_lo * dir_1 + lin_hi * dir_2);
        let near2far1 = get_light(lin_hi * dir_1 + lin_lo * dir_2);
        let mut far12 = get_light(lin_hi * dir_1 + lin_hi * dir_2);

        if !near1far2.valid() && !near2far1.valid() {
            // The far corner is on the other side of a diagonal wall, so should be
            // ignored to prevent light leaks.
            far12 = near12;
        }

        // Apply ambient occlusion.
        let near12 = near12.value_with_ambient_occlusion();
        let near1far2 = near1far2.value_with_ambient_occlusion();
        let near2far1 = near2far1.value_with_ambient_occlusion();
        let far12 = far12.value_with_ambient_occlusion();

        // Perform bilinear interpolation.
        fn mix(x: Vector4<f32>, y: Vector4<f32>, a: FreeCoordinate) -> Vector4<f32> {
            // This should be replaced with https://doc.rust-lang.org/nightly/std/primitive.f32.html#method.lerp when that's stable
            let a = a as f32;
            x * (1. - a) + y * a
        }
        let v = mix(
            mix(near12, near1far2, mix_2),
            mix(near2far1, far12, mix_2),
            mix_1,
        );
        Rgb::try_from(v.truncate() / v.w.max(0.1)).unwrap()
    }
}

impl<P: PixelBuf<Pixel = String>> SpaceRaytracer<P> {
    /// Raytrace to text, using any [`PixelBuf`] whose output is [`String`].
    ///
    /// `F` is the function accepting the output, and `E` is the type of error it may
    /// produce. This function-based interface is intended to abstract over the
    /// inconvenient difference between [`std::io::Write`] and [`std::fmt::Write`].
    ///
    /// After each line (row) of the image, `write(line_ending)` will be called.
    pub fn trace_scene_to_text<F, E>(
        &self,
        camera: &Camera,
        line_ending: &str,
        write: F,
    ) -> Result<RaytraceInfo, E>
    where
        F: FnMut(&str) -> Result<(), E>,
    {
        // This wrapper function ensures that the two implementations have consistent
        // signatures.
        self.trace_scene_to_text_impl(camera, line_ending, write)
    }

    #[cfg(feature = "rayon")]
    fn trace_scene_to_text_impl<F, E>(
        &self,
        camera: &Camera,
        line_ending: &str,
        mut write: F,
    ) -> Result<RaytraceInfo, E>
    where
        F: FnMut(&str) -> Result<(), E>,
    {
        let viewport = camera.viewport();
        let viewport_size = viewport.framebuffer_size.map(|s| s as usize);
        let output_iterator = (0..viewport_size.y)
            .into_par_iter()
            .map(move |ych| {
                let y = viewport.normalize_fb_y(ych);
                (0..viewport_size.x)
                    .into_par_iter()
                    .map(move |xch| {
                        let x = viewport.normalize_fb_x(xch);
                        self.trace_ray(camera.project_ndc_into_world(Point2::new(x, y)))
                    })
                    .chain(Some((line_ending.to_owned(), RaytraceInfo::default())).into_par_iter())
            })
            .flatten();

        let (text, info_sum): (String, rayon_helper::ParExtSum<RaytraceInfo>) =
            output_iterator.unzip();
        write(text.as_ref())?;

        Ok(info_sum.result())
    }

    #[cfg(not(feature = "rayon"))]
    fn trace_scene_to_text_impl<F, E>(
        &self,
        camera: &Camera,
        line_ending: &str,
        mut write: F,
    ) -> Result<RaytraceInfo, E>
    where
        F: FnMut(&str) -> Result<(), E>,
    {
        let mut total_info = RaytraceInfo::default();

        let viewport = camera.viewport();
        let viewport_size = viewport.framebuffer_size.map(|s| s as usize);
        for ych in 0..viewport_size.y {
            let y = viewport.normalize_fb_y(ych);
            for xch in 0..viewport_size.x {
                let x = viewport.normalize_fb_x(xch);
                let (text, info) = self.trace_ray(camera.project_ndc_into_world(Point2::new(x, y)));
                total_info += info;
                write(text.as_ref())?;
            }
            write(line_ending)?;
        }

        Ok(total_info)
    }
}

/// Performance info from a [`SpaceRaytracer`] operation.
///
/// The contents of this structure are subject to change; use [`Debug`] to view it.
/// The [`Default`] value is the zero value.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct RaytraceInfo {
    cubes_traced: usize,
}
impl std::ops::AddAssign<RaytraceInfo> for RaytraceInfo {
    fn add_assign(&mut self, other: Self) {
        self.cubes_traced += other.cubes_traced;
    }
}
impl std::iter::Sum for RaytraceInfo {
    fn sum<I>(iter: I) -> Self
    where
        I: Iterator<Item = Self>,
    {
        let mut sum = Self::default();
        for part in iter {
            sum += part;
        }
        sum
    }
}

/// Print an image of the given space as “ASCII art”.
///
/// Intended for use in tests, to visualize the results in case of failure.
/// Accordingly, it always writes to the same destination as [`print!`] (which is
/// redirected when tests are run).
///
/// `direction` specifies the direction from which the camera will be looking towards
/// the center of the space. The text output will be 80 columns wide.
pub fn print_space(space: &Space, direction: impl Into<Vector3<FreeCoordinate>>) {
    print_space_impl(space, direction, |s| {
        print!("{}", s);
    });
}

/// Version of `print_space` that takes a destination, for testing.
fn print_space_impl<F: FnMut(&str)>(
    space: &Space,
    direction: impl Into<Vector3<FreeCoordinate>>,
    mut write: F,
) -> RaytraceInfo {
    // TODO: optimize height (and thus aspect ratio) for the shape of the space
    let mut camera = Camera::new(
        GraphicsOptions::default(),
        Viewport {
            nominal_size: Vector2::new(40., 40.),
            framebuffer_size: Vector2::new(80, 40),
        },
    );
    camera.set_view_matrix(Matrix4::look_at_rh(
        eye_for_look_at(space.grid(), direction.into()),
        space.grid().center(),
        Vector3::new(0., 1., 0.),
    ));

    SpaceRaytracer::<CharacterBuf>::new(space, GraphicsOptions::default())
        .trace_scene_to_text(&camera, &"\n", move |s| {
            write(s);
            let r: Result<(), ()> = Ok(());
            r
        })
        .unwrap()
}

/// Get block data out of [`Space`] (which is not [`Sync`], and not specialized for our
/// efficient use).
#[inline]
fn prepare_blocks<P: PixelBuf>(space: &Space) -> Box<[TracingBlock<P::BlockData>]> {
    space
        .block_data()
        .iter()
        .map(|block_data| {
            let evaluated = block_data.evaluated();
            let pixel_block_data = P::compute_block_data(block_data);
            if let Some(ref voxels) = evaluated.voxels {
                TracingBlock::Recur(pixel_block_data, evaluated.resolution, voxels.clone())
            } else {
                TracingBlock::Atom(pixel_block_data, evaluated.color)
            }
        })
        .collect()
}

/// Get cube data out of [`Space`] (which is not [`Sync`], and not specialized for our
/// efficient use).
#[inline]
#[allow(clippy::ptr_arg)] // no benefit
fn prepare_cubes<'a, P: PixelBuf>(
    indexed_block_data: &'a [TracingBlock<P::BlockData>],
    space: &Space,
) -> GridArray<TracingCubeData<'a, P::BlockData>> {
    space.extract(space.grid(), |index, _block, lighting| TracingCubeData {
        block: &indexed_block_data[index.unwrap() as usize],
        lighting,
    })
}

#[derive(Clone, Debug)]
struct TracingCubeData<'a, B: 'static> {
    block: &'a TracingBlock<B>,
    lighting: PackedLight,
}

#[derive(Clone, Debug)]
enum TracingBlock<B: 'static> {
    Atom(B, Rgba),
    Recur(B, Resolution, GridArray<Evoxel>),
}

#[derive(Clone, Debug, Default)]
struct TracingState<P: PixelBuf> {
    /// Number of cubes traced through -- controlled by the caller, so not necessarily
    /// equal to the number of calls to [`Self::trace_through_surface()`].
    cubes_traced: usize,
    pixel_buf: P,
}
impl<P: PixelBuf> TracingState<P> {
    #[inline]
    fn count_step_should_stop(&mut self) -> bool {
        self.cubes_traced += 1;
        if self.cubes_traced > 1000 {
            // Abort excessively long traces.
            self.pixel_buf = Default::default();
            self.pixel_buf
                .add(Rgba::new(1.0, 1.0, 1.0, 1.0), &P::error_block_data());
            true
        } else {
            self.pixel_buf.opaque()
        }
    }

    fn finish(mut self, sky_color: Rgb) -> (P::Pixel, RaytraceInfo) {
        if self.cubes_traced == 0 {
            // Didn't intersect the world at all. Draw these as plain background.
            // TODO: Switch to using the sky color, unless debugging options are set.
            self.pixel_buf.hit_nothing();
        }

        self.pixel_buf
            .add(sky_color.with_alpha_one(), &P::sky_block_data());

        (
            self.pixel_buf.result(),
            RaytraceInfo {
                cubes_traced: self.cubes_traced,
            },
        )
    }

    /// Apply the effect of a given surface color.
    ///
    /// Note this is not true volumetric ray tracing: we're considering each
    /// voxel surface to be discrete.
    #[inline]
    fn trace_through_surface(
        &mut self,
        block_data: &P::BlockData,
        surface: Rgba,
        lighting: Rgb,
        face: Face,
        options: &GraphicsOptions,
    ) {
        let surface = options.transparency.limit_alpha(surface);
        if surface.fully_transparent() {
            return;
        }
        let adjusted_rgb = surface.to_rgb() * lighting * fixed_directional_lighting(face);
        self.pixel_buf
            .add(adjusted_rgb.with_alpha(surface.alpha()), block_data);
    }
}

/// Simple directional lighting used to give corners extra definition.
/// Note that this algorithm is also implemented in the fragment shader for GPU rendering.
fn fixed_directional_lighting(face: Face) -> f32 {
    let normal = face.normal_vector();
    const LIGHT_1_DIRECTION: Vector3<f32> = Vector3::new(0.4, -0.1, 0.0);
    const LIGHT_2_DIRECTION: Vector3<f32> = Vector3::new(-0.4, 0.35, 0.25);
    (1.0 - 1.0 / 16.0)
        + 0.25 * (LIGHT_1_DIRECTION.dot(normal).max(0.0) + LIGHT_2_DIRECTION.dot(normal).max(0.0))
}

/// Implementations of [`PixelBuf`] define output formats of the raytracer, by being
/// responsible for accumulating the color (and/or other information) for each image
/// pixel.
///
/// They should be an efficiently updatable buffer able to accumulate partial values,
/// and it must represent the transparency so as to be able to signal when to stop
/// tracing.
///
/// The implementation of the [`Default`] trait must provide a suitable initial state,
/// i.e. fully transparent/no light accumulated.
pub trait PixelBuf: Default {
    /// Type of the pixel value this [`PixelBuf`] produces; the value that will be
    /// returned by tracing a single ray.
    ///
    /// This trait does not define how multiple pixels are combined into an image.
    type Pixel: Send + Sync + 'static;

    /// Type of the data precomputed for each distinct block by
    /// [`Self::compute_block_data()`].
    ///
    /// If no data beyond color is needed, this may be `()`.
    // Note: I tried letting BlockData contain references but I couldn't satisfy
    // the borrow checker.
    type BlockData: Send + Sync + 'static;

    /// Computes whatever data this [`PixelBuf`] wishes to have available in
    /// [`Self::add`], for a given block.
    fn compute_block_data(block: &SpaceBlockData) -> Self::BlockData;

    /// Computes whatever value should be passed to [`Self::add`] when the raytracer
    /// encounters an error.
    fn error_block_data() -> Self::BlockData;

    /// Computes whatever value should be passed to [`Self::add`] when the raytracer
    /// encounters the sky (background behind all blocks).
    fn sky_block_data() -> Self::BlockData;

    /// Returns whether `self` has recorded an opaque surface and therefore will not
    /// be affected by future calls to [`Self::add`].
    fn opaque(&self) -> bool;

    /// Computes the value the raytracer should return for this pixel when tracing is
    /// complete.
    fn result(self) -> Self::Pixel;

    /// Adds the color of a surface to the buffer. The provided color should already
    /// have the effect of lighting applied.
    ///
    /// You should probably give this method the `#[inline]` attribute.
    ///
    /// TODO: this interface might want even more information; generalize it to be
    /// more future-proof.
    fn add(&mut self, surface_color: Rgba, block_data: &Self::BlockData);

    /// Indicates that the trace did not intersect any space that could have contained
    /// anything to draw. May be used for special diagnostic drawing. If used, should
    /// disable the effects of future [`Self::add`] calls.
    fn hit_nothing(&mut self) {}
}

/// Implements [`PixelBuf`] for RGB(A) color with [`f32`] components.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorBuf {
    /// Color buffer.
    ///
    /// The value can be interpreted as being “premultiplied alpha” value where the alpha
    /// is `1.0 - self.ray_alpha`, or equivalently we can say that it is the color to
    /// display supposing that everything not already traced is black.
    ///
    /// Note: Not using the [`Rgb`] type so as to skip NaN checks.
    color_accumulator: Vector3<f32>,

    /// Fraction of the color value that is to be determined by future, rather than past,
    /// tracing; starts at 1.0 and decreases as surfaces are encountered.
    ray_alpha: f32,
}

impl PixelBuf for ColorBuf {
    type Pixel = Rgba;
    type BlockData = ();

    fn compute_block_data(_: &SpaceBlockData) {}

    fn error_block_data() {}

    fn sky_block_data() {}

    #[inline]
    fn result(self) -> Rgba {
        if self.ray_alpha >= 1.0 {
            // Special case to avoid dividing by zero
            Rgba::TRANSPARENT
        } else {
            let color_alpha = 1.0 - self.ray_alpha;
            let non_premultiplied_color = self.color_accumulator / color_alpha;
            Rgba::try_from(non_premultiplied_color.extend(color_alpha))
                .unwrap_or_else(|_| Rgba::new(1.0, 0.0, 0.0, 1.0))
        }
    }

    #[inline]
    fn opaque(&self) -> bool {
        // Let's suppose that we don't care about differences that can't be represented
        // in 8-bit color...not considering gamma.
        self.ray_alpha < 1.0 / 256.0
    }

    #[inline]
    fn add(&mut self, surface_color: Rgba, _block_data: &Self::BlockData) {
        let color_vector: Vector3<f32> = surface_color.to_rgb().into();
        let surface_alpha = surface_color.alpha().into_inner();
        let alpha_for_add = surface_alpha * self.ray_alpha;
        self.ray_alpha *= 1.0 - surface_alpha;
        self.color_accumulator += color_vector * alpha_for_add;
    }
}

impl Default for ColorBuf {
    #[inline]
    fn default() -> Self {
        Self {
            color_accumulator: Vector3::zero(),
            ray_alpha: 1.0,
        }
    }
}

/// Implements [`PixelBuf`] for text output: captures the first characters of block names
/// rather than colors.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharacterBuf {
    /// Text to draw, if determined yet.
    hit_text: Option<String>,
}

impl PixelBuf for CharacterBuf {
    type Pixel = String;
    type BlockData = Cow<'static, str>;

    fn compute_block_data(s: &SpaceBlockData) -> Self::BlockData {
        // TODO: For more Unicode correctness, index by grapheme cluster...
        // ...and do something clever about double-width characters.
        s.evaluated()
            .attributes
            .display_name
            .chars()
            .next()
            .map(|c| Cow::Owned(c.to_string()))
            .unwrap_or(Cow::Borrowed(&" "))
    }

    fn error_block_data() -> Self::BlockData {
        Cow::Borrowed(&"X")
    }

    fn sky_block_data() -> Self::BlockData {
        Cow::Borrowed(&" ")
    }

    #[inline]
    fn opaque(&self) -> bool {
        self.hit_text.is_some()
    }

    #[inline]
    fn result(self) -> String {
        self.hit_text.unwrap_or_else(|| ".".to_owned())
    }

    #[inline]
    fn add(&mut self, _surface_color: Rgba, text: &Self::BlockData) {
        if self.hit_text.is_none() {
            self.hit_text = Some(text.to_owned().to_string());
        }
    }

    fn hit_nothing(&mut self) {
        self.hit_text = Some(".".to_owned());
    }
}

#[cfg(feature = "rayon")]
mod rayon_helper {
    use rayon::iter::{IntoParallelIterator, ParallelExtend, ParallelIterator as _};
    use std::iter::{empty, once, Sum};

    /// Implements [`ParallelExtend`] to just sum things, so that
    /// [`ParallelIterator::unzip`] can produce a sum.
    #[cfg(feature = "rayon")]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct ParExtSum<T>(Option<T>);

    #[cfg(feature = "rayon")]
    impl<T: Sum> ParExtSum<T> {
        pub fn result(self) -> T {
            self.0.unwrap_or_else(|| empty().sum())
        }
    }

    #[cfg(feature = "rayon")]
    impl<T: Sum + Send> ParallelExtend<T> for ParExtSum<T> {
        fn par_extend<I>(&mut self, par_iter: I)
        where
            I: IntoParallelIterator<Item = T>,
        {
            let new = par_iter.into_par_iter().sum();
            // The reason we use an `Option` at all is to make it possible to move the current
            // value.
            self.0 = Some(match self.0.take() {
                None => new,
                Some(previous) => once(previous).chain(once(new)).sum(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::content::make_some_blocks;
    use crate::universe::Universe;
    // use ordered_float::NotNan;

    #[test]
    fn color_buf() {
        let color_1 = Rgba::new(1.0, 0.0, 0.0, 0.75);
        let color_2 = Rgba::new(0.0, 1.0, 0.0, 0.5);
        let color_3 = Rgba::new(0.0, 0.0, 1.0, 1.0);

        let mut buf = ColorBuf::default();
        assert_eq!(buf.clone().result(), Rgba::TRANSPARENT);
        assert!(!buf.opaque());

        buf.add(color_1, &());
        assert_eq!(buf.clone().result(), color_1);
        assert!(!buf.opaque());

        buf.add(color_2, &());
        // TODO: this is not the right assertion because it's the premultiplied form.
        // assert_eq!(
        //     buf.result(),
        //     (color_1.to_rgb() * 0.75 + color_2.to_rgb() * 0.125)
        //         .with_alpha(NotNan::new(0.875).unwrap())
        // );
        assert!(!buf.opaque());

        buf.add(color_3, &());
        assert!(buf.clone().result().fully_opaque());
        //assert_eq!(
        //    buf.result(),
        //    (color_1.to_rgb() * 0.75 + color_2.to_rgb() * 0.125 + color_3.to_rgb() * 0.125)
        //        .with_alpha(NotNan::one())
        //);
        assert!(buf.opaque());
    }

    // TODO: test actual raytracer
    // Particularly, test subcube/voxel rendering

    #[test]
    fn print_space_test() {
        let mut space = Space::empty_positive(3, 1, 1);
        let [b0, b1, b2] = make_some_blocks();
        space.set((0, 0, 0), &b0).unwrap();
        space.set((1, 0, 0), &b1).unwrap();
        space.set((2, 0, 0), &b2).unwrap();

        let mut output = String::new();
        print_space_impl(&space, (1., 1., 1.), |s| output += s);
        print!("{}", output);
        assert_eq!(
            output,
            "\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ...........................0000000000...........................................\n\
            .......................0000000000000001111......................................\n\
            ........................000000001111111111111111................................\n\
            ........................00000011111111111111111112222...........................\n\
            .........................0000011111111111111122222222222222.....................\n\
            .........................000001111111111112222222222222222222222................\n\
            ...........................000011111111122222222222222222222222222..............\n\
            .............................001111111112222222222222222222222222...............\n\
            ...............................111111111222222222222222222222222................\n\
            ..................................11111122222222222222222222222.................\n\
            ....................................11112222222222222222222222..................\n\
            .......................................1222222222222222222222...................\n\
            .........................................2222222222222222222....................\n\
            ............................................222222222222222.....................\n\
            ..............................................22222222222.......................\n\
            ................................................22222222........................\n\
            ...................................................2222.........................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
            ................................................................................\n\
        "
        );
    }

    /// Check that blocks with small spaces are handled without out-of-bounds errors
    #[test]
    fn partial_voxels() {
        let resolution = 4;
        let mut universe = Universe::new();
        let mut block_space = Space::empty_positive(4, 2, 4);
        block_space
            .fill_uniform(block_space.grid(), Block::from(Rgba::WHITE))
            .unwrap();
        let space_ref = universe.insert_anonymous(block_space);
        let partial_block = Block::builder()
            .voxels_ref(resolution as Resolution, space_ref.clone())
            .display_name("P")
            .build();

        let mut space = Space::empty_positive(2, 1, 1);
        let [b0] = make_some_blocks();
        space.set([0, 0, 0], &b0).unwrap();
        space.set([1, 0, 0], &partial_block).unwrap();

        let mut output = String::new();
        print_space_impl(&space, (1., 1., 1.), |s| output += s);
        print!("{}", output);
        assert_eq!(
            output,
            "\
                ................................................................................\n\
                ................................................................................\n\
                ................................................................................\n\
                ................................................................................\n\
                ................................................................................\n\
                ................................................................................\n\
                ................................................................................\n\
                ................................................................................\n\
                ................................................................................\n\
                ................................................................................\n\
                ................................................................................\n\
                ................................................................................\n\
                ...............................000000...........................................\n\
                ......................0000000000000000000000....................................\n\
                ...................0000000000000000000000000000    .............................\n\
                ....................000000000000000000000000000           ......................\n\
                ....................000000000000000000000000000                  ...............\n\
                .....................0000000000000000000000000                       ...........\n\
                ......................00000000000000000000000PP                      ...........\n\
                ......................00000000000000000000PPPPPPPPPP                ............\n\
                .......................000000000000000PPPPPPPPPPPPPPPPPP           .............\n\
                .......................000000000000PPPPPPPPPPPPPPPPPPPPPPPPPP     ..............\n\
                .........................0000000PPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPP...............\n\
                ............................0000PPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPP................\n\
                ..............................00PPPPPPPPPPPPPPPPPPPPPPPPPPPPPPP.................\n\
                ................................0PPPPPPPPPPPPPPPPPPPPPPPPPPPPP..................\n\
                ..................................PPPPPPPPPPPPPPPPPPPPPPPPPPP...................\n\
                ....................................PPPPPPPPPPPPPPPPPPPPPPP.....................\n\
                ......................................PPPPPPPPPPPPPPPPPPPP......................\n\
                ........................................PPPPPPPPPPPPPPPPP.......................\n\
                ..........................................PPPPPPPPPPPPP.........................\n\
                ............................................PPPPPPPPPP..........................\n\
                ..............................................PPPPPP............................\n\
                ................................................PPP.............................\n\
                ................................................................................\n\
                ................................................................................\n\
                ................................................................................\n\
                ................................................................................\n\
                ................................................................................\n\
                ................................................................................\n\
            "
        );
    }
}
