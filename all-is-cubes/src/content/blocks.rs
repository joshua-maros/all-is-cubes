// Copyright 2020-2021 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <http://opensource.org/licenses/MIT>.

//! Block definitions that are specific to the demo/initial content and not fundamental
//! or UI.

use noise::Seedable as _;

use crate::block::{Block, Resolution, AIR};
use crate::blockgen::LandscapeBlocks;
use crate::linking::{BlockModule, BlockProvider};
use crate::math::{int_magnitude_squared, GridPoint, NoiseFnExt as _, NotNan, RGB, RGBA};
use crate::universe::{InsertError, Universe};

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, strum::Display, strum::EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum DemoBlocks {
    Lamp,
    Lamppost,
    Road,
}
impl BlockModule for DemoBlocks {
    fn namespace() -> &'static str {
        "all-is-cubes/demo-blocks"
    }
}

/// Add to `universe` demo-content blocks, that might be used by demo worldgen or offered to the player.
pub fn install_demo_blocks(universe: &mut Universe) -> Result<(), InsertError> {
    let resolution = 16;
    install_landscape_blocks(universe, resolution)?;

    use DemoBlocks::*;
    let stone_color: Block = RGBA::new(0.4, 0.4, 0.4, 1.0).into();
    let road_noise_v = noise::Value::new().set_seed(0x51b19f6a);
    let road_noise = noise::ScaleBias::new(&road_noise_v)
        .set_bias(1.0)
        .set_scale(0.04);
    BlockProvider::<DemoBlocks>::new(|key| match key {
        Road => Block::builder()
            .display_name("Cobblestone")
            .voxels_fn(universe, resolution, |cube| {
                scale_color(stone_color.clone(), road_noise.at_grid(cube), 0.02)
            })
            .unwrap()
            .build(),

        Lamp => Block::builder()
            .display_name("Lamp")
            .light_emission(RGB::new(20.0, 20.0, 20.0))
            .voxels_fn(universe, resolution, |p| {
                if int_magnitude_squared(p - GridPoint::new(8, 8, 8)) <= 8 * 8 + 3
                /* fudge */
                {
                    RGBA::WHITE.into()
                } else {
                    AIR.clone()
                }
            })
            .unwrap()
            .build(),

        Lamppost => Block::builder()
            .display_name("Lamppost")
            .light_emission(RGB::new(3.0, 3.0, 3.0))
            .voxels_fn(universe, resolution, |p| {
                let central = p - GridPoint::new(8, 8, 8);
                if (central.x.pow(2) + central.z.pow(2)) < 2 * 2 {
                    RGBA::BLACK.into()
                } else {
                    AIR.clone()
                }
            })
            .unwrap()
            .build(),
    })
    .install(universe)?;

    Ok(())
}

/// Construct blocks for [`LandscapeBlocks`] with some detail and add block definitions to the universe.
// TODO: not sure if we want this to be a public interface; is currently in use by lighting_bench
#[doc(hidden)]
pub fn install_landscape_blocks(
    universe: &mut Universe,
    resolution: Resolution,
) -> Result<(), InsertError> {
    use LandscapeBlocks::*;
    let colors = BlockProvider::<LandscapeBlocks>::default();

    let stone_noise_v = noise::Value::new().set_seed(0x21b5cc6b);
    let stone_noise = noise::ScaleBias::new(&stone_noise_v)
        .set_bias(1.0)
        .set_scale(0.04);
    let dirt_noise_v = noise::Value::new().set_seed(0x2e240365);
    let dirt_noise = noise::ScaleBias::new(&dirt_noise_v)
        .set_bias(1.0)
        .set_scale(0.12);
    let overhang_noise_v = noise::Value::new();
    let overhang_noise = noise::ScaleBias::new(&overhang_noise_v)
        .set_bias(f64::from(resolution) * 0.75)
        .set_scale(2.5);

    BlockProvider::<LandscapeBlocks>::new(|key| match key {
        Stone => Block::builder()
            .attributes(colors[Stone].evaluate().unwrap().attributes)
            .voxels_fn(universe, resolution, |cube| {
                scale_color((*colors[Stone]).clone(), stone_noise.at_grid(cube), 0.02)
            })
            .unwrap()
            .build(),

        Grass => Block::builder()
            .attributes(colors[Grass].evaluate().unwrap().attributes)
            .voxels_fn(universe, resolution, |cube| {
                if f64::from(cube.y) >= overhang_noise.at_grid(cube) {
                    scale_color((*colors[Grass]).clone(), dirt_noise.at_grid(cube), 0.02)
                } else {
                    scale_color((*colors[Dirt]).clone(), dirt_noise.at_grid(cube), 0.02)
                }
            })
            .unwrap()
            .build(),

        Dirt => Block::builder()
            .attributes(colors[Dirt].evaluate().unwrap().attributes)
            .voxels_fn(universe, resolution, |cube| {
                scale_color((*colors[Dirt]).clone(), dirt_noise.at_grid(cube), 0.02)
            })
            .unwrap()
            .build(),

        Trunk => (*colors[Trunk]).clone(),

        Leaves => (*colors[Leaves]).clone(),
    })
    .install(universe)?;
    Ok(())
}

/// Generate a copy of a [`Block::Atom`] with its color scaled by the given scalar.
///
/// The scalar is rounded to steps of `quantization`, to reduce the number of distinct
/// block types generated.
///
/// If the computation is NaN or the block is not an atom, it is returned unchanged.
fn scale_color(block: Block, scalar: f64, quantization: f64) -> Block {
    let scalar = (scalar / quantization).round() * quantization;
    match (block, NotNan::new(scalar as f32)) {
        (Block::Atom(attributes, color), Ok(scalar)) => Block::Atom(
            attributes,
            (color.to_rgb() * scalar).with_alpha(color.alpha()),
        ),
        (block, _) => block,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn install_demo_blocks_test() {
        let mut universe = Universe::new();
        install_demo_blocks(&mut universe).unwrap();
        // TODO: assert what entries were created, once Universe has iteration
    }
}
