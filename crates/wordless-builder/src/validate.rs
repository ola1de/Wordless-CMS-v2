//! P11 Visual Builder — layout validation.
//!
//! A builder layout is Elementor-compatible JSON stored under the
//! `_elementor_data` postmeta key: an array of sections, each holding
//! columns, each holding widgets. Validation is structural only —
//! unknown widget types are NOT errors (an Elementor layout imported
//! from WordPress may contain widgets this renderer does not know;
//! they render as nothing and are reported as `unsupported` at
//! preview/render time, exactly like the P3 renderer).

use serde_json::Value;

/// A layout that passed structural validation, plus the widget types
/// it contains that no renderer here knows (reported, never an error).
#[derive(Debug)]
pub struct ValidLayout {
    /// Unknown widget types, first-seen order, deduplicated.
    pub unsupported: Vec<String>,
    /// Total node count (sections + columns + widgets).
    pub nodes: usize,
}

const MAX_NODES: usize = 500;
const MAX_DEPTH: usize = 10;
const MAX_CHILDREN: usize = 100;

/// Validate a layout value. `Ok` carries the unknown widget types;
/// `Err` carries the structural problems (all of them, not just the
/// first — the builder shows the whole list).
pub fn validate_layout(data: &Value) -> Result<ValidLayout, Vec<String>> {
    let mut errors: Vec<String> = Vec::new();
    let mut unsupported: Vec<String> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    let Some(arr) = data.as_array() else {
        return Err(vec!["layout must be an array of sections".to_string()]);
    };
    if arr.is_empty() {
        return Err(vec!["layout must contain at least one section".to_string()]);
    }
    let mut nodes = 0usize;
    for (i, el) in arr.iter().enumerate() {
        validate_element(el, 0, &format!("sections[{i}]"), &mut errors, &mut unsupported, &mut ids, &mut nodes);
    }
    if nodes > MAX_NODES {
        errors.push(format!(
            "layout has {nodes} nodes; at most {MAX_NODES} are allowed"
        ));
    }
    if errors.is_empty() {
        Ok(ValidLayout { unsupported, nodes })
    } else {
        Err(errors)
    }
}

fn validate_element(
    el: &Value,
    depth: usize,
    path: &str,
    errors: &mut Vec<String>,
    unsupported: &mut Vec<String>,
    ids: &mut Vec<String>,
    nodes: &mut usize,
) {
    *nodes += 1;
    if depth > MAX_DEPTH {
        errors.push(format!("{path}: nested too deep (max {MAX_DEPTH})"));
        return;
    }
    let Some(obj) = el.as_object() else {
        errors.push(format!("{path}: every element must be an object"));
        return;
    };
    if let Some(id) = obj.get("id").and_then(|v| v.as_str()) {
        if id.is_empty() {
            errors.push(format!("{path}: id must not be empty"));
        } else if ids.contains(&id.to_string()) {
            errors.push(format!("{path}: duplicate id {id:?}"));
        } else {
            ids.push(id.to_string());
        }
    }
    let el_type = obj.get("elType").and_then(|v| v.as_str()).unwrap_or("");
    let children: Vec<&Value> = obj
        .get("elements")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    if children.len() > MAX_CHILDREN {
        errors.push(format!(
            "{path}: {n} children; at most {MAX_CHILDREN} are allowed",
            n = children.len()
        ));
    }
    match el_type {
        "section" => {
            for (i, child) in children.iter().enumerate() {
                let child_type = child
                    .get("elType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if child_type != "column" {
                    errors.push(format!(
                        "{path}: a section may only contain columns, found {child_type:?} at elements[{i}]"
                    ));
                }
                validate_element(
                    child,
                    depth + 1,
                    &format!("{path}.elements[{i}]"),
                    errors,
                    unsupported,
                    ids,
                    nodes,
                );
            }
        }
        "column" => {
            for (i, child) in children.iter().enumerate() {
                let child_type = child
                    .get("elType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if child_type != "widget" {
                    errors.push(format!(
                        "{path}: a column may only contain widgets, found {child_type:?} at elements[{i}]"
                    ));
                }
                validate_element(
                    child,
                    depth + 1,
                    &format!("{path}.elements[{i}]"),
                    errors,
                    unsupported,
                    ids,
                    nodes,
                );
            }
        }
        "widget" => {
            let widget_type = obj.get("widgetType").and_then(|v| v.as_str()).unwrap_or("");
            if widget_type.is_empty() {
                errors.push(format!("{path}: a widget needs a widgetType"));
            } else if !crate::palette::is_core_widget(widget_type)
                && !crate::palette::is_element_widget(widget_type)
                && !unsupported.contains(&widget_type.to_string())
            {
                // Unknown here only means "not a built-in type" — an
                // extension widget or an imported Elementor widget.
                // It is reported, not an error.
                unsupported.push(widget_type.to_string());
            }
            if let Some(settings) = obj.get("settings") {
                if !settings.is_object() {
                    errors.push(format!("{path}: settings must be an object"));
                }
            }
            // Widgets do not nest in Elementor; children, if any, are
            // still walked so a bad node inside one is caught.
            for (i, child) in children.iter().enumerate() {
                validate_element(
                    child,
                    depth + 1,
                    &format!("{path}.elements[{i}]"),
                    errors,
                    unsupported,
                    ids,
                    nodes,
                );
            }
        }
        other => {
            errors.push(format!("{path}: unknown elType {other:?}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn section(widget_type: &str) -> Value {
        json!([
            {
                "id": "s1", "elType": "section",
                "elements": [
                    {
                        "id": "c1", "elType": "column",
                        "elements": [
                            { "id": "w1", "elType": "widget", "widgetType": widget_type, "settings": {} }
                        ]
                    }
                ]
            }
        ])
    }

    #[test]
    fn valid_core_and_element_layouts_pass() {
        for t in ["heading", "wordless-badge", "wordless-table"] {
            let v = validate_layout(&section(t)).expect("layout should be valid");
            assert!(v.unsupported.is_empty(), "{t} should be known");
            assert_eq!(v.nodes, 3);
        }
    }

    #[test]
    fn unknown_widget_type_is_reported_not_an_error() {
        let v = validate_layout(&section("fancy-slider")).expect("layout should be valid");
        assert_eq!(v.unsupported, vec!["fancy-slider".to_string()]);
    }

    #[test]
    fn structural_problems_are_all_reported() {
        // Not an array.
        assert!(validate_layout(&json!({"nope": true})).is_err());
        // Empty array.
        assert!(validate_layout(&json!([])).is_err());
        // Widget directly under a section.
        let bad = json!([
            { "id": "s1", "elType": "section", "elements": [
                { "id": "w1", "elType": "widget", "widgetType": "heading", "settings": {} }
            ] }
        ]);
        let errs = validate_layout(&bad).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("only contain columns")));
        // Duplicate ids.
        let dup = json!([
            { "id": "s1", "elType": "section", "elements": [
                { "id": "c1", "elType": "column", "elements": [
                    { "id": "w1", "elType": "widget", "widgetType": "heading", "settings": {} },
                    { "id": "w1", "elType": "widget", "widgetType": "heading", "settings": {} }
                ] }
            ] }
        ]);
        let errs = validate_layout(&dup).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("duplicate id")));
        // Widget without a type.
        let no_type = json!([
            { "id": "s1", "elType": "section", "elements": [
                { "id": "c1", "elType": "column", "elements": [
                    { "id": "w1", "elType": "widget", "settings": {} }
                ] }
            ] }
        ]);
        let errs = validate_layout(&no_type).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("needs a widgetType")));
    }
}
