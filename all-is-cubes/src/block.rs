// Copyright 2020 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <http://opensource.org/licenses/MIT>.

//! Definition of blocks, which are game objects which live in the grid of a
//! [`Space`]. See [`Block`] for details.

use cgmath::EuclideanSpace as _;
use std::borrow::Cow;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use crate::listen::{Gate, Listener, ListenerHelper, Notifier};
use crate::math::{GridCoordinate, GridPoint, RGB, RGBA};
use crate::space::{Grid, GridArray, Space, SpaceChange};
use crate::universe::{Name, RefError, URef, Universe, UniverseIndex as _};
use crate::util::ConciseDebug;

/// Type for the edge length of recursive blocks in terms of their component voxels.
/// This resolution cubed is the number of voxels making up a block.
///
/// This type was chosen as `u8` so as to make it nonnegative and easy to losslessly
/// convert into larger, possibly signed, sizes. It's plenty of range since a resolution
/// of 255 would mean 16 million voxels — more than we want to work with.
pub type Resolution = u8;

/// A `Block` is something that can exist in the grid of a [`Space`]; it occupies one unit
/// cube of space and has a specified appearance and behavior.
///
/// In general, when a block appears multiple times from an in-game perspective, that may
/// or may not be the the same copy; `Block`s are "by value". However, some blocks are
/// defined by reference to shared mutable data, in which case changes to that data should
/// take effect everywhere a `Block` having that same reference occurs.
///
/// To obtain the concrete appearance and behavior of a block, use [`Block::evaluate`] to
/// obtain an [`EvaluatedBlock`] value, preferably with caching.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum Block {
    /// A block whose definition is stored in a [`Universe`](crate::universe::Universe).
    Indirect(URef<BlockDef>),

    /// A block that is a single-colored unit cube. (It may still be be transparent or
    /// non-solid to physics.)
    Atom(BlockAttributes, RGBA),

    /// A block that is composed of smaller blocks, defined by the referenced `Space`.
    Recur {
        attributes: BlockAttributes,
        /// Which portion of the space will be used, specified by the most negative
        /// corner.
        offset: GridPoint,
        /// The side length of the cubical volume of sub-blocks (voxels) used for this
        /// block.
        resolution: u8,
        space: URef<Space>,
    },
}

impl Block {
    /// Returns a new [`BlockBuilder`] which may be used to construct a [`Block`] value
    /// from various inputs with convenient syntax.
    pub const fn builder() -> BlockBuilder<builder::NeedsColorOrVoxels> {
        builder::DEFAULT
    }

    /// Converts this `Block` into a “flattened” and snapshotted form which contains all
    /// information needed for rendering and physics, and does not require [`URef`] access
    /// to other objects.
    pub fn evaluate(&self) -> Result<EvaluatedBlock, RefError> {
        match self {
            Block::Indirect(def_ref) => def_ref.try_borrow()?.block.evaluate(),

            Block::Atom(attributes, color) => Ok(EvaluatedBlock {
                attributes: attributes.clone(),
                color: *color,
                voxels: None,
                opaque: color.fully_opaque(),
                visible: !color.fully_transparent(),
            }),

            Block::Recur {
                attributes,
                offset,
                resolution,
                space: space_ref,
            } => {
                // Ensure resolution is at least 1 to not panic on bad data.
                // (We could eliminate this if Grid allowed a size of zero, but that
                // might lead to division-by-zero trouble elsewhere...)
                let resolution: GridCoordinate = (*resolution).max(1).into();
                let offset = *offset;

                let block_space = space_ref.try_borrow()?;
                let grid = Grid::new(offset, (resolution, resolution, resolution));
                let voxels = block_space
                    .extract(grid, |_index, sub_block_data, _lighting| {
                        // TODO: need to also extract solidity info once we start doing collision
                        sub_block_data.evaluated().color
                    })
                    .translate(-offset.to_vec());
                Ok(EvaluatedBlock {
                    attributes: attributes.clone(),
                    color: RGBA::new(0.5, 0.5, 0.5, 1.0), // TODO replace this with averaging the voxels
                    // TODO wrong test: we want to see if the _faces_ are all opaque but allow hollows
                    opaque: voxels
                        .grid()
                        .interior_iter()
                        .all(|p| voxels[p].fully_opaque()),
                    visible: voxels
                        .grid()
                        .interior_iter()
                        .any(|p| !voxels[p].fully_transparent()),

                    voxels: Some(voxels),
                })
            }
        }
        // TODO: need to track which things we need change notifications on
    }

    /// Registers a listener for mutations of any data sources which may affect this
    /// block's [`Block::evaluate`] result.
    ///
    /// Note that this does not listen for mutations of the `Block` value itself —
    /// which would be impossible since it is an enum and all its fields
    /// are public. In contrast, [`BlockDef`] does perform such tracking.
    ///
    /// This may fail under the same conditions as `evaluate`.
    pub fn listen(&self, listener: impl Listener<BlockChange> + 'static) -> Result<(), RefError> {
        match self {
            Block::Indirect(def_ref) => {
                def_ref.try_borrow_mut()?.listen(listener)?;
            }
            Block::Atom(_, _) => {
                // Atoms don't refer to anything external and thus cannot change other
                // than being directly overwritten, which is out of the scope of this
                // operation.
            }
            Block::Recur {
                space: space_ref, ..
            } => {
                space_ref
                    .try_borrow_mut()?
                    .listen(listener.filter(|msg| match msg {
                        SpaceChange::Block(_) => Some(BlockChange::new()),
                        SpaceChange::BlockValue(_) => Some(BlockChange::new()),
                        SpaceChange::Lighting(_) => None,
                        SpaceChange::Number(_) => None,
                    }));
            }
        }
        Ok(())
    }

    /// Returns the single [RGBA] color of this block, or panics if it does not have a
    /// single color. For use in tests only.
    #[cfg(test)]
    pub fn color(&self) -> RGBA {
        match self {
            Block::Atom(_, c) => *c,
            _ => panic!("Block::color not defined for non-atom blocks"),
        }
    }
}

// Implementing conversions to `Cow` allow various functions to accept either an owned
// or borrowed `Block`. The motivation for this is to avoid unnecessary cloning
// (in case an individual block has large data).

impl From<Block> for Cow<'_, Block> {
    fn from(block: Block) -> Self {
        Cow::Owned(block)
    }
}
impl<'a> From<&'a Block> for Cow<'a, Block> {
    fn from(block: &'a Block) -> Self {
        Cow::Borrowed(block)
    }
}
/// Convert a color to a block with default attributes.
impl From<RGB> for Block {
    fn from(color: RGB) -> Self {
        Block::from(color.with_alpha_one())
    }
}
/// Convert a color to a block with default attributes.
impl From<RGBA> for Block {
    fn from(color: RGBA) -> Self {
        Block::Atom(BlockAttributes::default(), color)
    }
}
/// Convert a color to a block with default attributes.
impl From<RGB> for Cow<'_, Block> {
    fn from(color: RGB) -> Self {
        Cow::Owned(Block::from(color))
    }
}
/// Convert a color to a block with default attributes.
impl From<RGBA> for Cow<'_, Block> {
    fn from(color: RGBA) -> Self {
        Cow::Owned(Block::from(color))
    }
}

/// Collection of miscellaneous attribute data for blocks that doesn't come in variants.
///
/// `BlockAttributes::default()` will produce a reasonable set of defaults for “ordinary”
/// blocks.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub struct BlockAttributes {
    /// The name that should be displayed to players.
    ///
    /// The default value is the empty string. The empty string should be considered a
    /// reasonable choice for solid-color blocks with no special features.
    pub display_name: Cow<'static, str>,

    /// Whether players' cursors target it or pass through it.
    ///
    /// The default value is `true`.
    pub selectable: bool,

    /// Whether the block is a physical obstacle.
    ///
    /// The default value is `true`.
    pub solid: bool,

    /// Light emitted by the block.
    ///
    /// The default value is [`RGB::ZERO`].
    pub light_emission: RGB,
    // TODO: add 'behavior' functionality, if we don't come up with something else
}

const DEFAULT_ATTRIBUTES: BlockAttributes = BlockAttributes {
    display_name: Cow::Borrowed(""),
    selectable: true,
    solid: true,
    light_emission: RGB::ZERO,
};

impl Default for BlockAttributes {
    /// Block attributes suitable as default values for in-game use.
    fn default() -> BlockAttributes {
        DEFAULT_ATTRIBUTES
    }
}

/// Generic 'empty'/'null' block. It is used by [`Space`] to respond to out-of-bounds requests.
///
/// See also [`AIR_EVALUATED`].
pub const AIR: Block = Block::Atom(AIR_ATTRIBUTES, RGBA::TRANSPARENT);

/// The result of <code>[AIR].[evaluate()](Block::evaluate)</code>, as a constant.
/// This may be used when an [`EvaluatedBlock`] value is needed but there is no block
/// value.
///
/// ```
/// use all_is_cubes::block::{AIR, AIR_EVALUATED};
///
/// assert_eq!(Ok(AIR_EVALUATED), AIR.evaluate());
/// ```
pub const AIR_EVALUATED: EvaluatedBlock = EvaluatedBlock {
    attributes: AIR_ATTRIBUTES,
    color: RGBA::TRANSPARENT,
    voxels: None,
    opaque: false,
    visible: false,
};

const AIR_ATTRIBUTES: BlockAttributes = BlockAttributes {
    display_name: Cow::Borrowed("<air>"),
    selectable: false,
    solid: false,
    light_emission: RGB::ZERO,
};

/// A “flattened” and snapshotted form of [`Block`] which contains all information needed
/// for rendering and physics, and does not require dereferencing [`URef`]s.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvaluatedBlock {
    /// The block's attributes.
    pub attributes: BlockAttributes,
    /// The block's color; if made of multiple voxels, then an average or representative
    /// color.
    pub color: RGBA,
    /// The voxels making up the block, if any; if [`None`], then `self.color` should be
    /// used as a uniform color value.
    ///
    /// TODO: Specify how it should be handled if the grid has unsuitable dimensions
    /// (not cubical, not having an origin of 0, etc.).
    pub voxels: Option<GridArray<RGBA>>,
    /// Whether the block is known to be completely opaque to light on all six faces.
    ///
    /// Currently, this is defined to be that each of the surfaces of the block are
    /// fully opaque, but in the future it might be refined to permit concave surfaces.
    // TODO: generalize opaque to multiple faces and partial opacity, for better light transport
    pub opaque: bool,
    /// Whether the block has any voxels/color at all that make it visible; that is, this
    /// is false if the block is completely transparent.
    pub visible: bool,
}

impl ConciseDebug for EvaluatedBlock {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        fmt.debug_struct("EvaluatedBlock")
            .field("attributes", &self.attributes)
            .field("color", &self.color)
            .field("opaque", &self.opaque)
            .field("visible", &self.visible)
            .field("voxels", &"...")
            .finish()
    }
}

/// Notification when an [`EvaluatedBlock`] result changes.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub struct BlockChange {
    /// I expect there _might_ be future uses for a set of flags of what changed;
    /// this helps preserve the option of adding them.
    _not_public: (),
}

impl BlockChange {
    #[allow(clippy::new_without_default)]
    pub fn new() -> BlockChange {
        BlockChange { _not_public: () }
    }
}

/// Contains a [`Block`] and can be stored in a [`Universe`](crate::universe::Universe).
/// Together with [`Block::Indirect`], this allows mutation of a block definition such
/// that all its usages follow.
///
/// It is a distinct type from [`Block`] in order to ensure that change notifications
/// will be delivered on any mutation.
#[derive(Debug)]
pub struct BlockDef {
    block: Block,
    // TODO: It might be a good idea to cache EvaluatedBlock here, since we're doing
    // mutation tracking anyway.
    notifier: Rc<Notifier<BlockChange>>,
    block_listen_gate: Gate,
}

impl BlockDef {
    pub fn new(block: Block) -> Self {
        let notifier = Rc::new(Notifier::new());
        let (gate, block_listener) = Notifier::forwarder(Rc::downgrade(&notifier)).gate();
        // TODO: Log if listening fails. We can't meaningfully fail this because we want to do the
        // parallel operation in `BlockDefMut::drop` but it does indicate trouble if it happens.
        let _ = block.listen(block_listener);
        BlockDef {
            block,
            notifier,
            block_listen_gate: gate,
        }
    }

    /// Registers a listener for mutations of any data sources which may affect the
    /// [`Block::evaluate`] result from blocks defined using this block definition.
    pub fn listen(
        &mut self,
        listener: impl Listener<BlockChange> + 'static,
    ) -> Result<(), RefError> {
        // TODO: Need to arrange listening to the contained block, and either translate
        // that here or have our own notifier generate forwardings.
        self.notifier.listen(listener);
        Ok(())
    }

    /// Creates a handle by which the contained block may be mutated.
    ///
    /// When the handle is dropped, a change notification will be sent.
    pub fn modify(&mut self) -> BlockDefMut<'_> {
        BlockDefMut(self)
    }
}

/// Mutable borrow of the [`Block`] inside a [`BlockDefMut`].
///
/// Provides the functionality of delivering change notifications when mutations are
/// complete.
pub struct BlockDefMut<'a>(&'a mut BlockDef);

impl Deref for BlockDefMut<'_> {
    type Target = Block;
    fn deref(&self) -> &Self::Target {
        &self.0.block
    }
}
impl DerefMut for BlockDefMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0.block
    }
}
impl Drop for BlockDefMut<'_> {
    fn drop(&mut self) {
        let block_def = &mut self.0;

        // Swap out what we're listening to
        let (gate, block_listener) = Notifier::forwarder(Rc::downgrade(&block_def.notifier)).gate();
        let _ = block_def.block.listen(block_listener);
        block_def.block_listen_gate = gate; // old gate is now dropped

        block_def.notifier.notify(BlockChange::new());
    }
}

/// Construct a set of [`Block::Recur`] that form a miniature of the given `space`.
/// The returned [`Space`] contains each of the blocks; its coordinates will correspond to
/// those of the input, scaled down by `resolution`.
///
/// TODO: add doc test for this
pub fn space_to_blocks(
    resolution: Resolution,
    attributes: BlockAttributes,
    space_ref: URef<Space>,
) -> Result<Space, RefError> {
    let resolution_g: GridCoordinate = resolution.into();
    let source_grid = space_ref.try_borrow()?.grid();
    let destination_grid = source_grid.divide(resolution_g);

    let mut destination_space = Space::empty(destination_grid);
    destination_space
        .fill(destination_grid, move |cube| {
            Some(Block::Recur {
                attributes: attributes.clone(),
                offset: GridPoint::from_vec(cube.to_vec() * resolution_g),
                resolution,
                space: space_ref.clone(),
            })
        })
        .expect("can't happen: space_to_blocks failed to write to its own output space");
    Ok(destination_space)
}

#[doc(inline)]
pub use builder::BlockBuilder;

/// Lesser-used helpers for [`BlockBuilder`].
pub mod builder {
    use crate::space::SetCubeError;

    use super::*;

    pub(crate) const DEFAULT: BlockBuilder<NeedsColorOrVoxels> = BlockBuilder {
        attributes: DEFAULT_ATTRIBUTES,
        content: NeedsColorOrVoxels,
    };

    /// Tool for constructing [`Block`] values conveniently.
    ///
    /// To create one, call [`Block::builder()`].
    /// ([`BlockBuilder::default()`] is also available.)
    ///
    /// ```
    /// use all_is_cubes::block::Block;
    /// use all_is_cubes::math::RGBA;
    /// use std::borrow::Cow;
    ///
    /// let block = Block::builder()
    ///    .display_name("BROWN")
    ///    .color(RGBA::new(0.5, 0.5, 0., 1.))
    ///    .build();
    ///
    /// assert_eq!(block.evaluate().unwrap().color, RGBA::new(0.5, 0.5, 0., 1.));
    /// assert_eq!(
    ///     block.evaluate().unwrap().attributes.display_name,
    ///     Cow::Borrowed("BROWN"),
    /// );
    /// ```
    #[derive(Clone, Eq, PartialEq)]
    pub struct BlockBuilder<C> {
        attributes: BlockAttributes,
        content: C,
    }

    impl Default for BlockBuilder<NeedsColorOrVoxels> {
        fn default() -> Self {
            DEFAULT
        }
    }

    impl<C> BlockBuilder<C> {
        // TODO: When #![feature(const_precise_live_drops)] becomes stable, we can make
        // this builder usable in const contexts.
        // https://github.com/rust-lang/rust/issues/73255
        // Doing that will also require creating non-trait-using alternate methods,
        // until const traits https://github.com/rust-lang/rust/issues/67792 is also available.

        // TODO: Document all of these methods

        /// Sets the value for [`BlockAttributes::display_name`].
        pub fn attributes(mut self, value: BlockAttributes) -> Self {
            self.attributes = value;
            self
        }

        /// Sets the value for [`BlockAttributes::display_name`].
        pub fn display_name(mut self, value: impl Into<Cow<'static, str>>) -> Self {
            self.attributes.display_name = value.into();
            self
        }

        /// Sets the value for [`BlockAttributes::selectable`].
        pub const fn selectable(mut self, value: bool) -> Self {
            self.attributes.selectable = value;
            self
        }

        /// Sets the value for [`BlockAttributes::solid`].
        pub const fn solid(mut self, value: bool) -> Self {
            self.attributes.solid = value;
            self
        }

        /// Sets the value for [`BlockAttributes::light_emission`].
        pub fn light_emission(mut self, value: impl Into<RGB>) -> Self {
            self.attributes.light_emission = value.into();
            self
        }

        /// Sets the color value for building a [`Block::Atom`].
        ///
        /// This will replace any previous color **or voxels.**
        pub fn color(self, color: impl Into<RGBA>) -> BlockBuilder<RGBA> {
            BlockBuilder {
                attributes: self.attributes,
                content: color.into(),
            }
        }

        /// Sets the space for building a [`Block::Recur`].
        ///
        /// This will replace any previous voxels **or color.**
        pub fn voxels_ref(
            self,
            resolution: Resolution,
            space: URef<Space>,
        ) -> BlockBuilder<BlockBuilderVoxels> {
            BlockBuilder {
                attributes: self.attributes,
                content: BlockBuilderVoxels {
                    space,
                    resolution,
                    offset: GridPoint::origin(),
                },
            }
        }

        /// Constructs a `Space` for building a [`Block::Recur`], and calls
        /// the given function to fill it with blocks, in the manner of [`Space::fill`].
        ///
        /// Note that the resulting builder is cloned, all clones will share the same
        /// space.
        // TODO: (doc) test for this
        pub fn voxels_fn<F, B>(
            self,
            // TODO: awkward requirement for universe; defer computation instead, or
            // add a `.universe()` method to provide it once.
            universe: &mut Universe,
            // TODO: Maybe resolution should be a separate method? Check usage patterns later.
            resolution: Resolution,
            mut function: F,
        ) -> Result<BlockBuilder<BlockBuilderVoxels>, SetCubeError>
        where
            F: FnMut(GridPoint) -> B,
            B: std::borrow::Borrow<Block>,
        {
            let grid = Grid::for_block(resolution);
            let mut space = Space::empty(grid);
            space.fill(grid, |point| Some(function(point)))?;
            Ok(self.voxels_ref(resolution, universe.insert_anonymous(space)))
        }

        /// Converts this builder into a block value.
        pub fn build(self) -> Block
        where
            C: BuilderContentIndependent,
        {
            self.content.build_i(self.attributes)
        }

        /// Converts this builder into a block value and stores it as a [`BlockDef`] in
        /// the given [`Universe`] with the given name, then returns a [`Block::Indirect`]
        /// referring to it.
        pub fn into_named_definition(
            self,
            universe: &mut Universe,
            name: impl Into<Name>,
        ) -> Result<Block, crate::universe::InsertError>
        where
            C: BuilderContentInUniverse,
        {
            let block = self.content.build_u(self.attributes, universe);
            let def_ref = universe.insert(name.into(), BlockDef::new(block))?;
            Ok(Block::Indirect(def_ref))
        }
    }

    /// Voxel-specific builder methods.
    impl BlockBuilder<BlockBuilderVoxels> {
        pub fn offset(mut self, offset: GridPoint) -> Self {
            self.content.offset = offset;
            self
        }

        // TODO: It might be useful to have "offset equal to resolution"
        // and "add offset", but don't add those until use cases are seen.
    }

    /// Allows implicitly converting `BlockBuilder` to the block it would build.
    impl<C: BuilderContentIndependent> From<BlockBuilder<C>> for Block {
        fn from(builder: BlockBuilder<C>) -> Self {
            builder.build()
        }
    }
    /// Equivalent to `Block::builder().color(color)`.
    impl From<RGBA> for BlockBuilder<RGBA> {
        fn from(color: RGBA) -> Self {
            Block::builder().color(color)
        }
    }
    /// Equivalent to `Block::builder().color(color.with_alpha_one())`.
    impl From<RGB> for BlockBuilder<RGBA> {
        fn from(color: RGB) -> Self {
            Block::builder().color(color.with_alpha_one())
        }
    }

    /// Placeholder type for an incomplete [`BlockBuilder`]'s content. The builder
    /// cannot create an actual block until this is replaced.
    #[derive(Copy, Clone, Debug, Default, Eq, Hash, PartialEq)]
    pub struct NeedsColorOrVoxels;
    /// Content of a [`BlockBuilder`] that can build a block without a [`Universe`].
    pub trait BuilderContentIndependent {
        fn build_i(self, attributes: BlockAttributes) -> Block;
    }
    /// Content of a [`BlockBuilder`] that can only build a block with a [`Universe`].
    pub trait BuilderContentInUniverse {
        fn build_u(self, attributes: BlockAttributes, universe: &mut Universe) -> Block;
    }
    /// Every `BuilderContentIndependent` can act as `BuilderContentInUniverse`.
    impl<T: BuilderContentIndependent> BuilderContentInUniverse for T {
        fn build_u(self, attributes: BlockAttributes, _: &mut Universe) -> Block {
            self.build_i(attributes)
        }
    }
    /// Used by [`BlockBuilder::color`].
    impl BuilderContentIndependent for RGBA {
        fn build_i(self, attributes: BlockAttributes) -> Block {
            Block::Atom(attributes, self)
        }
    }

    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    pub struct BlockBuilderVoxels {
        space: URef<Space>,
        resolution: Resolution,
        offset: GridPoint,
    }
    impl BuilderContentIndependent for BlockBuilderVoxels {
        fn build_i(self, attributes: BlockAttributes) -> Block {
            Block::Recur {
                attributes,
                offset: self.offset,
                resolution: self.resolution,
                space: self.space,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listen::Sink;
    use crate::math::{GridPoint, GridVector};
    use crate::space::Grid;
    use crate::universe::Universe;
    use std::borrow::Cow;

    #[test]
    fn evaluate_opaque_atom_and_attributes() {
        let color = RGBA::new(1.0, 2.0, 3.0, 1.0);
        let attributes = BlockAttributes {
            display_name: Cow::Borrowed(&"hello world"),
            selectable: false,
            solid: false,
            light_emission: RGB::ONE,
            ..BlockAttributes::default()
        };
        let block = Block::Atom(attributes.clone(), color);
        let e = block.evaluate().unwrap();
        assert_eq!(e.attributes, attributes);
        assert_eq!(e.color, block.color());
        assert!(e.voxels.is_none());
        assert_eq!(e.opaque, true);
        assert_eq!(e.visible, true);
    }

    #[test]
    fn evaluate_transparent_atom() {
        let color = RGBA::new(1.0, 2.0, 3.0, 0.5);
        let block = Block::Atom(BlockAttributes::default(), color);
        let e = block.evaluate().unwrap();
        assert_eq!(e.color, block.color());
        assert!(e.voxels.is_none());
        assert_eq!(e.opaque, false);
        assert_eq!(e.visible, true);
    }

    #[test]
    fn evaluate_invisible_atom() {
        let block = Block::Atom(BlockAttributes::default(), RGBA::TRANSPARENT);
        let e = block.evaluate().unwrap();
        assert_eq!(e.color, RGBA::TRANSPARENT);
        assert!(e.voxels.is_none());
        assert_eq!(e.opaque, false);
        assert_eq!(e.visible, false);
    }

    #[test]
    fn evaluate_voxels_checked_individually() {
        let resolution = 4;
        let mut universe = Universe::new();

        let attributes = BlockAttributes {
            display_name: Cow::Borrowed(&"hello world"),
            ..BlockAttributes::default()
        };
        let block = Block::builder()
            .attributes(attributes.clone())
            .voxels_fn(&mut universe, resolution, |point| {
                let point = point.cast::<f32>().unwrap();
                Block::from(RGBA::new(point.x, point.y, point.z, 1.0))
            })
            .unwrap()
            .build();

        let e = block.evaluate().unwrap();
        assert_eq!(e.attributes, attributes);
        assert_eq!(
            e.voxels,
            Some(GridArray::generate(Grid::for_block(resolution), |point| {
                let point = point.cast::<f32>().unwrap();
                RGBA::new(point.x, point.y, point.z, 1.0)
            }))
        );
        assert_eq!(e.opaque, true);
        assert_eq!(e.visible, true);
    }

    #[test]
    fn evaluate_transparent_voxels() {
        let mut universe = Universe::new();
        let resolution = 4;
        let block = Block::builder()
            .voxels_fn(&mut universe, resolution, |point| {
                Block::from(RGBA::new(
                    0.0,
                    0.0,
                    0.0,
                    if point == GridPoint::new(0, 0, 0) {
                        0.5
                    } else {
                        1.0
                    },
                ))
            })
            .unwrap()
            .build();

        let e = block.evaluate().unwrap();
        assert_eq!(e.opaque, false);
        assert_eq!(e.visible, true);
    }

    #[test]
    fn evaluate_voxels_not_filling_block() {
        let mut universe = Universe::new();
        let block = Block::builder()
            .voxels_fn(&mut universe, 4, |point| {
                Block::from(RGBA::new(
                    0.0,
                    0.0,
                    0.0,
                    if point == GridPoint::new(1, 1, 1) {
                        1.0
                    } else {
                        0.0
                    },
                ))
            })
            .unwrap()
            .build();

        let e = block.evaluate().unwrap();
        assert_eq!(e.opaque, false);
        assert_eq!(e.visible, true);
    }

    /// Tests that the `offset` field of `Block::Recur` is respected.
    #[test]
    fn recur_with_offset() {
        let resolution = 4;
        let offset = GridVector::new(resolution, 0, 0);
        let mut universe = Universe::new();
        let mut space = Space::empty_positive(resolution * 2, resolution, resolution);
        space
            .fill(space.grid(), |point| {
                let point = point.cast::<f32>().unwrap();
                Some(Block::Atom(
                    BlockAttributes::default(),
                    RGBA::new(point.x, point.y, point.z, 1.0),
                ))
            })
            .unwrap();
        let space_ref = universe.insert_anonymous(space);
        let block_at_offset = Block::Recur {
            attributes: BlockAttributes::default(),
            offset: GridPoint::from_vec(offset),
            resolution: resolution as Resolution,
            space: space_ref.clone(),
        };

        let e = block_at_offset.evaluate().unwrap();
        assert_eq!(
            e.voxels,
            Some(GridArray::generate(
                Grid::for_block(resolution as Resolution),
                |point| {
                    let point = (point + offset).cast::<f32>().unwrap();
                    RGBA::new(point.x, point.y, point.z, 1.0)
                }
            ))
        );
    }

    #[test]
    fn indirect_equivalence() {
        let resolution = 4;
        let mut universe = Universe::new();
        let mut space = Space::empty_positive(resolution, resolution, resolution);
        // TODO: BlockGen should support constructing indirects (by default, even)
        // and we can use the more concise version
        space
            .fill(space.grid(), |point| {
                let point = point.cast::<f32>().unwrap();
                Some(Block::from(RGBA::new(point.x, point.y, point.z, 1.0)))
            })
            .unwrap();
        let space_ref = universe.insert_anonymous(space);
        let block = Block::builder()
            .voxels_ref(resolution as Resolution, space_ref.clone())
            .build();
        let eval_bare = block.evaluate();
        let block_def_ref = universe.insert_anonymous(BlockDef::new(block));
        let eval_def = block_def_ref.borrow().block.evaluate();
        assert_eq!(eval_bare, eval_def);
    }

    #[test]
    fn listen_atom() {
        let block = Block::from(RGBA::WHITE);
        let mut sink = Sink::new();
        block.listen(sink.listener()).unwrap();
        assert_eq!(None, sink.next());
        // No notifications are possible, so nothing more to test.
    }

    #[test]
    fn listen_indirect_atom() {
        let mut universe = Universe::new();
        let block_def_ref = universe.insert_anonymous(BlockDef::new(Block::from(RGBA::WHITE)));
        let indirect = Block::Indirect(block_def_ref.clone());
        let mut sink = Sink::new();
        indirect.listen(sink.listener()).unwrap();
        assert_eq!(None, sink.next());

        // Now mutate it and we should see a notification.
        *(block_def_ref.borrow_mut().modify()) = Block::from(RGBA::BLACK);
        assert!(sink.next().is_some());
    }

    /// Testing double indirection not because it's a case we expect to use routinely,
    /// but because it exercises the generality of the notification mechanism.
    #[test]
    fn listen_indirect_double() {
        let mut universe = Universe::new();
        let block_def_ref1 = universe.insert_anonymous(BlockDef::new(Block::from(RGBA::WHITE)));
        let block_def_ref2 =
            universe.insert_anonymous(BlockDef::new(Block::Indirect(block_def_ref1.clone())));
        let indirect2 = Block::Indirect(block_def_ref2.clone());
        let mut sink = Sink::new();
        indirect2.listen(sink.listener()).unwrap();
        assert_eq!(None, sink.next());

        // Now mutate the original block and we should see a notification.
        *(block_def_ref1.borrow_mut().modify()) = Block::from(RGBA::BLACK);
        assert!(sink.next().is_some());

        // Remove block_def_ref1 from the contents of block_def_ref2...
        *(block_def_ref2.borrow_mut().modify()) = Block::from(RGBA::BLACK);
        assert!(sink.next().is_some());
        assert!(sink.next().is_none());
        // ...and then block_def_ref1's changes should NOT be forwarded.
        *(block_def_ref1.borrow_mut().modify()) = Block::from(RGBA::WHITE);
        assert!(sink.next().is_none());
    }

    /// Test that changes to a `Space` propagate to block listeners.
    #[test]
    fn listen_recur() {
        let mut universe = Universe::new();
        let space_ref = universe.insert_anonymous(Space::empty_positive(1, 1, 1));
        let block = Block::builder().voxels_ref(1, space_ref.clone()).build();
        let mut sink = Sink::new();
        block.listen(sink.listener()).unwrap();
        assert_eq!(None, sink.next());

        // Now mutate the space and we should see a notification.
        space_ref
            .borrow_mut()
            .set((0, 0, 0), Block::from(RGBA::new(0.1, 0.2, 0.3, 0.4)))
            .unwrap();
        assert!(sink.next().is_some());

        // TODO: Also test that we don't propagate lighting changes
    }

    // TODO: test of evaluate where the block's space is the wrong size

    #[test]
    fn builder_defaults() {
        let color = RGBA::new(0.1, 0.2, 0.3, 0.4);
        assert_eq!(
            Block::builder().color(color).build(),
            Block::Atom(BlockAttributes::default(), color),
        );
    }

    #[test]
    fn builder_every_field() {
        let color = RGBA::new(0.1, 0.2, 0.3, 0.4);
        let light_emission = RGB::new(0.1, 3.0, 0.1);
        assert_eq!(
            Block::builder()
                .display_name("hello world")
                .solid(false)
                .color(color)
                .selectable(false)
                .light_emission(light_emission)
                .build(),
            Block::Atom(
                BlockAttributes {
                    display_name: "hello world".into(),
                    solid: false,
                    selectable: false,
                    light_emission
                },
                color
            ),
        );
    }

    #[test]
    fn builder_voxels_from_space() {
        let mut universe = Universe::new();
        let space_ref = universe.insert_anonymous(Space::empty_positive(1, 1, 1));

        assert_eq!(
            Block::builder()
                .display_name("hello world")
                .voxels_ref(2, space_ref.clone())
                .build(),
            Block::Recur {
                attributes: BlockAttributes {
                    display_name: "hello world".into(),
                    ..BlockAttributes::default()
                },
                offset: GridPoint::origin(),
                resolution: 2, // not same as space size
                space: space_ref
            },
        );
    }
}
