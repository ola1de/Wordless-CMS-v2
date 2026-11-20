//! P11 Visual Builder — palette data.
//!
//! The palette IS the library, not a copy: the icon list and element
//! export list come from the generated `palette_data` (derived from
//! `admin/src/components/icons.tsx` and `elements.tsx` by
//! `scripts/gen-builder-palette.mjs`); the widget/element definitions
//! below reference those same sources. `scripts/check-builder-palette.mjs`
//! fails when any list drifts from the TS sources.

use serde_json::{json, Value};

pub use crate::palette_data::{BUILDER_ICONS, ELEMENT_EXPORTS};

/// One editable setting of a widget/element, rendered by the admin
/// settings panel from this schema (no per-widget UI code).
pub struct FieldDef {
    /// Settings key, dotted for nested objects (`image.url`).
    pub key: &'static str,
    pub label: &'static str,
    /// `text` | `textarea` | `url` | `number` | `checkbox` | `select` |
    /// `list-text` (one string per line) | `list-object` (rows of
    /// `options` keys) | `rows` (one row per line, cells split on `|`).
    pub kind: &'static str,
    /// `select` values, or `list-object` object keys.
    pub options: &'static [&'static str],
    pub placeholder: &'static str,
}

const fn f(
    key: &'static str,
    label: &'static str,
    kind: &'static str,
    options: &'static [&'static str],
    placeholder: &'static str,
) -> FieldDef {
    FieldDef {
        key,
        label,
        kind,
        options,
        placeholder,
    }
}

/// One palette entry: a widget or element the builder can drop onto
/// the canvas.
pub struct PaletteItem {
    /// The `widgetType` stored in the layout JSON.
    pub widget_type: &'static str,
    pub label: &'static str,
    /// An icon from the library (an `IconName` in `icons.tsx`).
    pub icon: &'static str,
    pub description: &'static str,
    /// The admin export this entry comes from (`elements.tsx`), or
    /// `""` for a core Elementor widget (no admin export).
    pub source_export: &'static str,
    pub fields: &'static [FieldDef],
}

/// The 10 core Elementor widgets the P3 renderer supports.
pub const CORE_WIDGETS: &[PaletteItem] = &[
    PaletteItem {
        widget_type: "heading",
        label: "Heading",
        icon: "typeHeading",
        description: "A title line, H1 to H6.",
        source_export: "",
        fields: &[
            f("title", "Title", "text", &[], "Welcome to Wordless"),
            f(
                "header_size",
                "Size",
                "select",
                &["h1", "h2", "h3", "h4", "h5", "h6"],
                "",
            ),
            f("align", "Alignment", "select", &["left", "center", "right"], ""),
        ],
    },
    PaletteItem {
        widget_type: "text-editor",
        label: "Text",
        icon: "fileText",
        description: "A paragraph of formatted text.",
        source_export: "",
        fields: &[f("editor", "Text", "textarea", &[], "Write something…")],
    },
    PaletteItem {
        widget_type: "image",
        label: "Image",
        icon: "image",
        description: "A picture, optionally linked and captioned.",
        source_export: "",
        fields: &[
            f("image.url", "Image URL", "url", &[], "https://…"),
            f("image.alt", "Alt text", "text", &[], "What the image shows"),
            f("caption", "Caption", "text", &[], ""),
            f("link.url", "Link URL", "url", &[], ""),
        ],
    },
    PaletteItem {
        widget_type: "button",
        label: "Button",
        icon: "link",
        description: "A call-to-action button or link.",
        source_export: "Button",
        fields: &[
            f("text", "Button text", "text", &[], "Get started"),
            f("link.url", "Link URL", "url", &[], "https://…"),
            f("align", "Alignment", "select", &["left", "center", "right"], ""),
        ],
    },
    PaletteItem {
        widget_type: "spacer",
        label: "Spacer",
        icon: "layers",
        description: "Empty vertical space.",
        source_export: "",
        fields: &[f("space.size", "Height (px)", "number", &[], "50")],
    },
    PaletteItem {
        widget_type: "divider",
        label: "Divider",
        icon: "layout",
        description: "A horizontal rule between sections.",
        source_export: "",
        fields: &[],
    },
    PaletteItem {
        widget_type: "video",
        label: "Video",
        icon: "video",
        description: "An embedded YouTube, Vimeo, Dailymotion or hosted video.",
        source_export: "",
        fields: &[
            f(
                "video_type",
                "Source",
                "select",
                &["youtube", "vimeo", "dailymotion", "hosted"],
                "",
            ),
            f("youtube_url", "YouTube URL", "url", &[], "https://www.youtube.com/watch?v=…"),
            f("vimeo_url", "Vimeo URL", "url", &[], ""),
            f("external_url", "Hosted video URL", "url", &[], ""),
        ],
    },
    PaletteItem {
        widget_type: "icon-list",
        label: "Icon List",
        icon: "list",
        description: "A list of items, each with an icon.",
        source_export: "",
        fields: &[f(
            "icon_list",
            "Items",
            "list-object",
            &["icon", "text"],
            "One item per row: icon name, then text",
        )],
    },
    PaletteItem {
        widget_type: "image-box",
        label: "Image Box",
        icon: "image",
        description: "An image with a title and description.",
        source_export: "",
        fields: &[
            f("image.url", "Image URL", "url", &[], "https://…"),
            f("image.alt", "Alt text", "text", &[], ""),
            f("title_text", "Title", "text", &[], ""),
            f("description_text", "Description", "textarea", &[], ""),
            f("link.url", "Link URL", "url", &[], ""),
        ],
    },
    PaletteItem {
        widget_type: "google-maps",
        label: "Map",
        icon: "globe",
        description: "An embedded Google map for an address.",
        source_export: "",
        fields: &[
            f("address", "Address", "text", &[], "Lagos, Nigeria"),
            f("zoom.size", "Zoom (1–21)", "number", &[], "12"),
            f("height.size", "Height (px)", "number", &[], "400"),
        ],
    },
];

/// The 26 shared admin elements, one palette entry per export in
/// `elements.tsx` — the full library, not a subset. Several exports
/// share one node type (Badge/Tag, Avatar/AvatarStack,
/// Toast/ToastProvider/useToast, Progress/ProgressRange,
/// CommandPalette/useCommandPaletteHotkey); each entry still appears
/// under its own name so the palette covers every export.
pub const ELEMENT_ITEMS: &[PaletteItem] = &[
    PaletteItem {
        widget_type: "button",
        label: "Button",
        icon: "link",
        description: "The shared Button element.",
        source_export: "Button",
        fields: &[
            f("text", "Button text", "text", &[], "Get started"),
            f("link.url", "Link URL", "url", &[], "https://…"),
            f("align", "Alignment", "select", &["left", "center", "right"], ""),
        ],
    },
    PaletteItem {
        widget_type: "wordless-input",
        label: "Input",
        icon: "pen",
        description: "A text field with a label.",
        source_export: "Input",
        fields: &[
            f("label", "Label", "text", &[], "Your name"),
            f("placeholder", "Placeholder", "text", &[], ""),
            f(
                "inputType",
                "Type",
                "select",
                &["text", "email", "tel", "number"],
                "",
            ),
            f("required", "Required", "checkbox", &[], ""),
        ],
    },
    PaletteItem {
        widget_type: "wordless-select",
        label: "Select",
        icon: "chevronDown",
        description: "A dropdown with options.",
        source_export: "Select",
        fields: &[
            f("label", "Label", "text", &[], "Pick one"),
            f(
                "options",
                "Options",
                "list-object",
                &["label", "value"],
                "One option per row: label, then value",
            ),
            f("required", "Required", "checkbox", &[], ""),
        ],
    },
    PaletteItem {
        widget_type: "wordless-toggle",
        label: "Toggle",
        icon: "check",
        description: "An on/off checkbox with a label.",
        source_export: "Toggle",
        fields: &[
            f("label", "Label", "text", &[], "Subscribe"),
            f("checked", "Checked", "checkbox", &[], ""),
        ],
    },
    PaletteItem {
        widget_type: "wordless-tabs",
        label: "Tabs",
        icon: "layers",
        description: "Tabbed panels; the first tab shows on the page.",
        source_export: "Tabs",
        fields: &[f(
            "tabs",
            "Tabs",
            "list-object",
            &["title", "content"],
            "One tab per row: title, then content",
        )],
    },
    PaletteItem {
        widget_type: "wordless-modal",
        label: "Modal",
        icon: "external",
        description: "A button that opens a dialog panel.",
        source_export: "Modal",
        fields: &[
            f("buttonText", "Button text", "text", &[], "Open"),
            f("title", "Dialog title", "text", &[], ""),
            f("content", "Dialog content", "textarea", &[], ""),
        ],
    },
    PaletteItem {
        widget_type: "wordless-drawer",
        label: "Drawer",
        icon: "menu",
        description: "A button that opens a side panel.",
        source_export: "Drawer",
        fields: &[
            f("buttonText", "Button text", "text", &[], "Open panel"),
            f("title", "Panel title", "text", &[], ""),
            f("content", "Panel content", "textarea", &[], ""),
        ],
    },
    PaletteItem {
        widget_type: "wordless-toast",
        label: "Toast",
        icon: "bell",
        description: "A short status notice.",
        source_export: "Toast",
        fields: &[
            f("message", "Message", "text", &[], "Saved."),
            f("tone", "Tone", "select", &["info", "ok", "warn", "err"], ""),
        ],
    },
    PaletteItem {
        widget_type: "wordless-toast",
        label: "Toast Provider",
        icon: "bell",
        description: "The toast host — inserts a toast notice.",
        source_export: "ToastProvider",
        fields: &[
            f("message", "Message", "text", &[], "Saved."),
            f("tone", "Tone", "select", &["info", "ok", "warn", "err"], ""),
        ],
    },
    PaletteItem {
        widget_type: "wordless-toast",
        label: "useToast",
        icon: "bell",
        description: "The toast hook — inserts a toast notice.",
        source_export: "useToast",
        fields: &[
            f("message", "Message", "text", &[], "Saved."),
            f("tone", "Tone", "select", &["info", "ok", "warn", "err"], ""),
        ],
    },
    PaletteItem {
        widget_type: "wordless-table",
        label: "Table",
        icon: "table",
        description: "A data table with headers and rows.",
        source_export: "Table",
        fields: &[
            f("headers", "Headers", "list-text", &[], "One header per line"),
            f("rows", "Rows", "rows", &[], "One row per line, cells separated by |"),
        ],
    },
    PaletteItem {
        widget_type: "wordless-badge",
        label: "Badge",
        icon: "star",
        description: "A small coloured label.",
        source_export: "Badge",
        fields: &[
            f("text", "Text", "text", &[], "New"),
            f(
                "tone",
                "Tone",
                "select",
                &["teal", "amber", "red", "blue", "grey"],
                "",
            ),
        ],
    },
    PaletteItem {
        widget_type: "wordless-badge",
        label: "Tag",
        icon: "priceTag",
        description: "A pill tag — the same badge element.",
        source_export: "Tag",
        fields: &[
            f("text", "Text", "text", &[], "New"),
            f(
                "tone",
                "Tone",
                "select",
                &["teal", "amber", "red", "blue", "grey"],
                "",
            ),
        ],
    },
    PaletteItem {
        widget_type: "wordless-avatar",
        label: "Avatar",
        icon: "user",
        description: "A person's initials in a circle, with a name.",
        source_export: "Avatar",
        fields: &[
            f("name", "Name", "text", &[], "Ada Lovelace"),
            f("subtitle", "Subtitle", "text", &[], "Author"),
        ],
    },
    PaletteItem {
        widget_type: "wordless-avatar",
        label: "Avatar Stack",
        icon: "users",
        description: "Overlapping avatars — inserts one avatar.",
        source_export: "AvatarStack",
        fields: &[
            f("name", "Name", "text", &[], "Ada Lovelace"),
            f("subtitle", "Subtitle", "text", &[], "Author"),
        ],
    },
    PaletteItem {
        widget_type: "wordless-breadcrumbs",
        label: "Breadcrumbs",
        icon: "chevronRight",
        description: "A trail of links back to the home page.",
        source_export: "Breadcrumbs",
        fields: &[f(
            "items",
            "Crumbs",
            "list-object",
            &["label", "href"],
            "One crumb per row: label, then link URL",
        )],
    },
    PaletteItem {
        widget_type: "wordless-stepper",
        label: "Stepper",
        icon: "clipboardCheck",
        description: "A sequence of steps, each done, current or todo.",
        source_export: "Stepper",
        fields: &[f(
            "steps",
            "Steps",
            "list-object",
            &["label", "state"],
            "One step per row: label, then done/current/todo",
        )],
    },
    PaletteItem {
        widget_type: "wordless-empty-state",
        label: "Empty State",
        icon: "info",
        description: "A friendly 'nothing here yet' block.",
        source_export: "EmptyState",
        fields: &[
            f("title", "Title", "text", &[], "Nothing here yet"),
            f("text", "Text", "textarea", &[], ""),
            f("buttonText", "Button text", "text", &[], ""),
            f("buttonLink.url", "Button link", "url", &[], ""),
        ],
    },
    PaletteItem {
        widget_type: "wordless-skeleton",
        label: "Skeleton",
        icon: "layers",
        description: "Loading placeholder bars.",
        source_export: "Skeleton",
        fields: &[f("lines", "Lines (1–8)", "number", &[], "3")],
    },
    PaletteItem {
        widget_type: "wordless-progress",
        label: "Progress",
        icon: "chart",
        description: "A progress bar with a label.",
        source_export: "Progress",
        fields: &[
            f("label", "Label", "text", &[], "Uploading"),
            f("value", "Value (0–100)", "number", &[], "50"),
        ],
    },
    PaletteItem {
        widget_type: "wordless-progress",
        label: "Progress Range",
        icon: "chart",
        description: "A progress bar with a range — same element.",
        source_export: "ProgressRange",
        fields: &[
            f("label", "Label", "text", &[], "Uploading"),
            f("value", "Value (0–100)", "number", &[], "50"),
        ],
    },
    PaletteItem {
        widget_type: "wordless-search",
        label: "Command Palette",
        icon: "search",
        description: "A search box — the command palette's visual form.",
        source_export: "CommandPalette",
        fields: &[f("placeholder", "Placeholder", "text", &[], "Search…")],
    },
    PaletteItem {
        widget_type: "wordless-search",
        label: "useCommandPaletteHotkey",
        icon: "search",
        description: "The palette hotkey — inserts a search box.",
        source_export: "useCommandPaletteHotkey",
        fields: &[f("placeholder", "Placeholder", "text", &[], "Search…")],
    },
    PaletteItem {
        widget_type: "wordless-card",
        label: "Card",
        icon: "layout",
        description: "A content card: image, title, text, button.",
        source_export: "Card",
        fields: &[
            f("title", "Title", "text", &[], ""),
            f("text", "Text", "textarea", &[], ""),
            f("image.url", "Image URL", "url", &[], "https://…"),
            f("image.alt", "Alt text", "text", &[], ""),
            f("buttonText", "Button text", "text", &[], ""),
            f("buttonLink.url", "Button link", "url", &[], ""),
        ],
    },
    PaletteItem {
        widget_type: "wordless-alert",
        label: "Alert",
        icon: "alert",
        description: "A notice box with a tone.",
        source_export: "Alert",
        fields: &[
            f("title", "Title", "text", &[], ""),
            f("text", "Text", "textarea", &[], ""),
            f("tone", "Tone", "select", &["ok", "warn", "err", "info"], ""),
        ],
    },
    PaletteItem {
        widget_type: "wordless-stat",
        label: "Stat Card",
        icon: "chart",
        description: "A big number with a label.",
        source_export: "StatCard",
        fields: &[
            f("value", "Value", "text", &[], "1,204"),
            f("label", "Label", "text", &[], "Readers"),
        ],
    },
];

/// The responsive preview devices (doc §6.4: review at 1440 / 768 /
/// 360 px — the builder canvas previews at exactly those widths).
pub struct DeviceDef {
    pub id: &'static str,
    pub label: &'static str,
    pub width: u32,
}

pub const BUILDER_DEVICES: &[DeviceDef] = &[
    DeviceDef {
        id: "desktop",
        label: "Desktop",
        width: 1440,
    },
    DeviceDef {
        id: "tablet",
        label: "Tablet",
        width: 768,
    },
    DeviceDef {
        id: "phone",
        label: "Phone",
        width: 360,
    },
];

/// True when `name` is an icon in the library.
pub fn icon_known(name: &str) -> bool {
    BUILDER_ICONS.iter().any(|(n, _)| *n == name)
}

/// True when `widget_type` is a core Elementor widget.
pub fn is_core_widget(widget_type: &str) -> bool {
    CORE_WIDGETS
        .iter()
        .any(|w| w.widget_type == widget_type)
}

/// True when `widget_type` is a builder-native element widget.
pub fn is_element_widget(widget_type: &str) -> bool {
    ELEMENT_ITEMS
        .iter()
        .any(|w| w.widget_type == widget_type)
}

/// The settings schema of one palette item as JSON (what the API
/// returns and the admin settings panel renders from).
pub fn item_json(item: &PaletteItem) -> Value {
    json!({
        "type": item.widget_type,
        "label": item.label,
        "icon": item.icon,
        "description": item.description,
        "sourceExport": item.source_export,
        "fields": item.fields.iter().map(|f| json!({
            "key": f.key,
            "label": f.label,
            "kind": f.kind,
            "options": f.options,
            "placeholder": f.placeholder,
        })).collect::<Vec<_>>(),
    })
}

/// The full palette as JSON: core widgets, elements (one entry per
/// admin export), icons with groups, and preview devices. Extension
/// widgets are added by the route (they are gated, not static).
pub fn palette_json() -> Value {
    json!({
        "widgets": CORE_WIDGETS.iter().map(item_json).collect::<Vec<_>>(),
        "elements": ELEMENT_ITEMS.iter().map(item_json).collect::<Vec<_>>(),
        "icons": BUILDER_ICONS.iter().map(|(name, group)| json!({
            "name": name,
            "group": group,
        })).collect::<Vec<_>>(),
        "devices": BUILDER_DEVICES.iter().map(|d| json!({
            "id": d.id,
            "label": d.label,
            "width": d.width,
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn palette_covers_every_element_export_exactly_once() {
        let listed: Vec<&str> = ELEMENT_ITEMS
            .iter()
            .map(|i| i.source_export)
            .collect();
        assert_eq!(listed.len(), ELEMENT_EXPORTS.len());
        let mut a: Vec<&str> = listed.clone();
        let mut b: Vec<&str> = ELEMENT_EXPORTS.to_vec();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "every elements.tsx export must appear exactly once");
    }

    #[test]
    fn palette_icons_are_all_known_and_unique() {
        assert_eq!(BUILDER_ICONS.len(), 104);
        let names: HashSet<&str> = BUILDER_ICONS.iter().map(|(n, _)| *n).collect();
        assert_eq!(names.len(), 104);
        for item in CORE_WIDGETS.iter().chain(ELEMENT_ITEMS.iter()) {
            assert!(
                icon_known(item.icon),
                "palette item {} uses unknown icon {}",
                item.widget_type,
                item.icon
            );
        }
    }

    #[test]
    fn element_widget_types_match_the_renderer() {
        // The renderer's list is the truth for what renders; the
        // palette must not offer a type the renderer drops. (The
        // wordless-elementor dev-dependency exists for this test.)
        for item in ELEMENT_ITEMS {
            let t = item.widget_type;
            assert!(
                wordless_elementor::is_supported_widget(t),
                "palette element {t} is not rendered by wordless-elementor"
            );
        }
        for item in CORE_WIDGETS {
            assert!(
                wordless_elementor::is_supported_widget(item.widget_type),
                "core widget {} is not rendered",
                item.widget_type
            );
        }
        assert_eq!(CORE_WIDGETS.len(), 10);
        assert_eq!(ELEMENT_ITEMS.len(), 26);
    }
}
