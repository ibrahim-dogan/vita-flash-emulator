// This lint is helpful, but right now we have too many instances of it.
// TODO: Remove this once all instances are fixed.
#![cfg_attr(feature = "rv_prof_ops", feature(core_intrinsics))]
#![cfg_attr(feature = "rv_prof_ops", allow(internal_features))]
#![allow(clippy::needless_pass_by_ref_mut)]
// This lint is good in theory, but in AVMs we often need to do `let x = args.get(0); let y = args.get(1);` etc.
// It'd make those much less readable and consistent.
#![allow(clippy::get_first)]

#[macro_use]
mod display_object;
pub use display_object::{StageAlign, StageDisplayState, StageScaleMode};

#[macro_use]
extern crate num_derive;

#[macro_use]
mod avm1;
mod avm2;
pub mod rv_clock;
pub use rv_prof;

/// RuffleVita: an `rv_prof` zone that only exists in profiling builds
/// (`rv_prof_ops`), for hot paths where even a few stores matter.
macro_rules! rv_deep_zone {
    ($zone:ident) => {
        #[cfg(feature = "rv_prof_ops")]
        let _rv_zone = crate::rv_prof::zone(crate::rv_prof::Zone::$zone);
    };
}
pub(crate) use rv_deep_zone;
#[cfg(feature = "rv_opstats")]
pub use avm2::opstats;
mod avm_rng;
mod binary_data;
pub mod bitmap;
pub mod buffer;
mod character;
pub mod context;
pub mod context_menu;
mod drawing;
mod ecma_conversions;
pub mod events;
pub mod focus_tracker;
mod font;
mod frame_lifecycle;
mod html;
mod input;
mod library;
pub mod limits;
pub mod loader;
mod local_connection;
mod locale;
mod net_connection;
mod orphan_manager;
pub mod pixel_bender;
mod player;
mod prelude;
pub mod sandbox;
pub mod socket;
mod streams;
pub mod string;
mod system_properties;
pub mod tag_utils;
pub mod timer;
mod types;
pub mod utils;
mod vminterface;
mod xml;

pub mod backend;
pub mod compatibility_rules;
pub mod config;
#[cfg(feature = "egui")]
pub mod debug_ui;
pub mod external;
pub mod i18n;
pub mod stub;

pub use context_menu::ContextMenuItem;
pub use events::PlayerEvent;
pub use font::{DefaultFont, FontFileData, FontQuery, FontType};
pub use indexmap;
pub use loader::LoadBehavior;
pub use player::{Player, PlayerBuilder, PlayerMode, PlayerRuntime, StaticCallstack};
pub use ruffle_render::backend::ViewportDimensions;
pub use swf;
pub use swf::Color;
pub use ttf_parser;

/// The newest Flash Player version known to Ruffle.
pub const NEWEST_PLAYER_VERSION: u8 = 51;

/// The default Flash Player version that Ruffle will emulate.
pub const DEFAULT_PLAYER_VERSION: u8 = 32;
