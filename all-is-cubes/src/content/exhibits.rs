// Copyright 2020-2021 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <https://opensource.org/licenses/MIT>.

//! Miscellanous demonstrations of capability and manual test-cases.
//! The exhibits defined in this file are combined into [`crate::content::demo_city`].

use cgmath::{
    Basis2, EuclideanSpace as _, InnerSpace as _, Rad, Rotation as _, Rotation2, Vector2, Vector3,
};
use embedded_graphics::fonts::{Font8x16, Text};
use embedded_graphics::geometry::Point;
use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::style::TextStyleBuilder;
use ordered_float::NotNan;

use crate::block::{space_to_blocks, Block, BlockAttributes, BlockCollision, AIR};
use crate::content::Exhibit;
use crate::drawing::draw_to_blocks;
use crate::math::{FreeCoordinate, GridCoordinate, GridPoint, GridRotation, GridVector, Rgb, Rgba};
use crate::space::{Grid, Space};

pub(crate) static DEMO_CITY_EXHIBITS: &[Exhibit] = &[
    Exhibit {
        name: "Transparency WIP",
        footprint: Grid::new_c([-3, 0, -3], [7, 5, 7]),
        factory: |this, _universe| {
            let mut space = Space::empty(this.footprint);

            let glass = Block::from(Rgba::new(0.9, 0.9, 0.9, 0.25));
            for rot in GridRotation::CLOCKWISE.iterate() {
                let windowpane = Grid::from_lower_upper([-1, 0, 3], [2, 5, 4]);
                space.fill(
                    windowpane
                        .transform(rot.to_positive_octant_matrix(1))
                        .unwrap(),
                    |_| Some(&glass),
                )?;
            }

            Ok(space)
        },
    },
    Exhibit {
        name: "Knot",
        footprint: Grid::new_c([-2, -2, -1], [5, 5, 3]),
        factory: |this, universe| {
            let resolution = 16;
            let toroidal_radius = 24.;
            let knot_split_radius = 9.;
            let strand_radius = 6.;
            let twists = 2.5;

            let mut drawing_space = Space::empty(this.footprint.multiply(resolution));
            let paint = Block::from(Rgba::new(0.9, 0.9, 0.9, 1.0));
            drawing_space.fill(drawing_space.grid(), |p| {
                // Measure from midpoint of odd dimension space
                let p = p - Vector3::new(1, 1, 1) * (resolution / 2);
                // Work in floating point
                let p = p.map(FreeCoordinate::from);

                let cylindrical = Vector2::new((p.x.powi(2) + p.y.powi(2)).sqrt(), p.z);
                let torus_cross_section = cylindrical - Vector2::new(toroidal_radius, 0.);
                let angle = Rad(p.x.atan2(p.y));
                let rotated_cross_section =
                    Basis2::from_angle(angle * twists).rotate_vector(torus_cross_section);
                let knot_center_1 = rotated_cross_section - Vector2::new(knot_split_radius, 0.);
                let knot_center_2 = rotated_cross_section + Vector2::new(knot_split_radius, 0.);

                if knot_center_1.magnitude() < strand_radius
                    || knot_center_2.magnitude() < strand_radius
                {
                    Some(&paint)
                } else {
                    None
                }
            })?;
            let space = space_to_blocks(
                16,
                BlockAttributes {
                    display_name: this.name.into(),
                    collision: BlockCollision::None,
                    ..BlockAttributes::default()
                },
                universe.insert_anonymous(drawing_space),
            )?;
            Ok(space)
        },
    },
    Exhibit {
        name: "Text",
        footprint: Grid::new_c([0, 0, 0], [9, 1, 1]),
        factory: |_, universe| {
            let space = draw_to_blocks(
                universe,
                16,
                8,
                BlockAttributes::default(),
                Text::new("Hello block world", Point::new(0, -16)).into_styled(
                    TextStyleBuilder::new(Font8x16)
                        .text_color(Rgb888::new(120, 100, 200))
                        .build(),
                ),
            )?;
            Ok(space)
        },
    },
    Exhibit {
        name: "Resolutions",
        footprint: Grid::new_c([0, 0, 0], [5, 2, 3]),
        factory: |this, universe| {
            let mut space = Space::empty(this.footprint);

            for (i, &resolution) in [1, 2, 3, 8, 16, 32].iter().enumerate() {
                let i = i as GridCoordinate;
                let location = GridPoint::new(i.rem_euclid(3) * 2, 0, i.div_euclid(3) * 2);
                space.set(
                    location,
                    Block::builder()
                        .voxels_fn(universe, resolution, |p| {
                            if p.x + p.y + p.z >= GridCoordinate::from(resolution) {
                                return AIR.clone();
                            }
                            let rescale = if resolution > 8 { 4 } else { 1 };
                            let color = Rgb::from(p.to_vec().map(|s| {
                                NotNan::new(
                                    (s / GridCoordinate::from(rescale)) as f32
                                        / f32::from(resolution / rescale - 1).max(1.),
                                )
                                .unwrap()
                            }));
                            Block::from(color)
                        })?
                        .build(),
                )?;

                space.set(
                    location + GridVector::unit_y(),
                    &draw_to_blocks(
                        universe,
                        16,
                        0,
                        BlockAttributes {
                            display_name: resolution.to_string().into(),
                            collision: BlockCollision::None,
                            ..BlockAttributes::default()
                        },
                        Text::new(&resolution.to_string(), Point::new(0, -16)).into_styled(
                            TextStyleBuilder::new(Font8x16)
                                .text_color(Rgb888::new(10, 10, 10))
                                .build(),
                        ),
                    )?[GridPoint::origin()],
                )?;
            }

            Ok(space)
        },
    },
    {
        const RADIUS: i16 = 5;
        const O: i16 = -RADIUS - 1;
        const S: u16 = (RADIUS as u16 + 1) * 2 + 1;
        Exhibit {
            name: "Visible chunk chart",
            footprint: Grid::new_c([O, O, O], [S, S, S]),
            factory: |_this, _universe| {
                use crate::chunking::{ChunkChart, CHUNK_SIZE_FREE};
                // TODO: Show more than one size.
                let chart = ChunkChart::new(CHUNK_SIZE_FREE * (RADIUS as FreeCoordinate) - 0.1);
                Ok(chart.visualization())
            },
        }
    },
];