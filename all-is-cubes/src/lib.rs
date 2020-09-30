// Copyright 2020 Kevin Reid under the terms of the MIT License as detailed
// in the accompanying file README.md or <http://opensource.org/licenses/MIT>.

//! All is Cubes is a game/engine for worlds made of cubical blocks, where the blocks
//! are themselves made of “smaller” blocks that define their appearance and behavior.
//!
//! This crate defines the world model, simulation rules and some utilities; for concrete
//! rendering and user interface, see the `all-is-cubes-server` crate.

#![allow(clippy::collapsible_if)]
#![warn(clippy::cast_lossless)]

// TODO: consider exporting individual symbols instead of the modules, because
// the modules are mostly per-data-type rather than being convenient usage bundles.
// Or have modules reexport by API consumer (world-builder versus renderer etc.)

pub mod block;
pub mod blockgen;
pub mod camera;
pub mod demo_content;
pub mod drawing;
mod lighting;
pub mod lum;
pub mod math;
mod physics;
pub mod raycast;
pub mod space;
pub mod triangulator;
pub mod universe;
pub mod util;
pub mod worldgen;
