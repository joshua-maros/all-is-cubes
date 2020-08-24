// Copyright 2020 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <http://opensource.org/licenses/MIT>.

//! Procedural block generation.

use crate::block::{AIR, Block, BlockAttributes};
use crate::math::{GridPoint, RGBA};
use crate::space::{Space};
use crate::universe::{Universe};

pub struct BlockGen<'a> {
    pub universe: &'a mut Universe,
    pub size: isize,
}

impl<'a> BlockGen<'a> {
    pub fn block_from_function(
        &mut self,
        attributes: BlockAttributes,
        f: impl Fn(&BlockGen, GridPoint) -> Block
    ) -> Block {
        let mut space = Space::empty_positive(self.size, self.size, self.size);
        for point in space.grid().interior_iter() {
            space.set(point, &f(self, point));
        }
        Block::Recur(attributes, self.universe.insert_anonymous(space))
    }
}

/// Generate some atom blocks with unspecified contents for testing.
///
/// ```
/// use all_is_cubes::blockgen::make_some_blocks;
/// assert_eq!(make_some_blocks(3).len(), 3);
/// ```
pub fn make_some_blocks(count: usize) -> Vec<Block> {
    // TODO: should this return an iterator? would anyone care?
    let mut vec :Vec<Block> = Vec::with_capacity(count);
    for i in 0..count {
        let luminance = if count > 1 {
            i as f32 / (count - 1) as f32
        } else {
            0.5
        };
        vec.push(Block::Atom(
            BlockAttributes {
                display_name: i.to_string().into(),
                ..BlockAttributes::default()
            },
            RGBA::new(luminance, luminance, luminance, 1.0)));
    }
    vec
}

/// A collection of block types assigned specific roles in generating outdoor landscapes.
pub struct LandscapeBlocks {
    pub air: Block,
    pub grass: Block,
    pub dirt: Block,
    pub stone: Block,
    pub trunk: Block,
    pub leaves: Block,
}

impl LandscapeBlocks {
    /// TODO: Improve and document
    pub fn new(ctx: &mut BlockGen) -> Self {
        let mut result = Self::default();
        let grass_color = result.grass.clone();
        let dirt_color = result.dirt.clone();

        // TODO: this needs to become a lot shorter
        result.grass = ctx.block_from_function(
            BlockAttributes {
                display_name: grass_color.attributes().display_name.clone(),
                ..BlockAttributes::default()
            },
            |ctx, point| {
                if point.y >= ctx.size - 1 {
                    grass_color.clone()
                } else {
                    dirt_color.clone()
                }
            });

        result
    }
}

impl Default for LandscapeBlocks {
    /// Generate a bland instance of `LandscapeBlocks` with single color blocks.
    fn default() -> LandscapeBlocks {
        fn color_and_name(r: f32, g: f32, b: f32, name: &str) -> Block {
            Block::Atom(
                BlockAttributes {
                    display_name: name.to_owned().into(),
                    ..BlockAttributes::default()
                },
                RGBA::new(r, g, b, 1.0))
        }
    
        LandscapeBlocks {
            air: AIR.clone(),
            grass: color_and_name(0.3, 0.8, 0.3, "Grass"),
            dirt: color_and_name(0.4, 0.2, 0.2, "Dirt"),
            stone: color_and_name(0.5, 0.5, 0.5, "Stone"),
            trunk: color_and_name(0.6, 0.3, 0.6, "Wood"),
            leaves: color_and_name(0.0, 0.7, 0.2, "Leaves"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // TODO: test block_from_function

    #[test]
    fn make_some_blocks_0() {
        assert_eq!(Vec::<Block>::new(), make_some_blocks(0));
    }

    #[test]
    fn make_some_blocks_1() {
        // should succeed even though the color range collapses range
        let blocks = make_some_blocks(1);
        assert_eq!(blocks[0].color(), RGBA::new(0.5, 0.5, 0.5, 1.0));
    }

    #[test]
    fn make_some_blocks_2() {
        let blocks = make_some_blocks(2);
        assert_eq!(blocks[0].color(), RGBA::new(0.0, 0.0, 0.0, 1.0));
        assert_eq!(blocks[1].color(), RGBA::new(1.0, 1.0, 1.0, 1.0));
    }
}