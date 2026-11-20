//! WORDLESS v2 P11 — Visual Builder.
//!
//! Palette data (the full admin element + icon libraries), layout
//! validation for Elementor-compatible JSON, and the extension-widget
//! registry. Pure logic over `wordless-db` — no Worker types.

pub mod palette;
pub mod palette_data;
pub mod registry;
pub mod validate;

pub use palette::{item_json, palette_json, BUILDER_DEVICES, BUILDER_ICONS, CORE_WIDGETS, ELEMENT_EXPORTS, ELEMENT_ITEMS};
pub use registry::{delete_widget, list_widgets, loaded_widgets, register_widget, valid_widget_type, BuilderWidget};
pub use validate::{validate_layout, ValidLayout};
