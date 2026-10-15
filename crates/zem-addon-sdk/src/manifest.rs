use crate::builders::Addon;

fn toml_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn serialize_operations(
    operations: &[crate::builders::PaneOperation],
    prefix: &str,
    toml: &mut String,
) {
    for operation in operations {
        toml.push_str(&format!(
            "\n{}\nname = \"{}\"\n",
            prefix,
            toml_string(&operation.name)
        ));
    }
}

/// Generate a TOML manifest from an Addon definition.
pub fn generate_manifest(addon: &Addon) -> String {
    let platforms = addon
        .platforms
        .iter()
        .map(|platform| format!("\"{}\"", toml_string(platform)))
        .collect::<Vec<_>>()
        .join(", ");
    let mut toml = format!(
        "id               = \"{}\"\nname             = \"{}\"\nversion          = \"{}\"\ndescription      = \"{}\"\nauthor           = \"{}\"\nmin_zem_version = \"{}\"\nplatforms        = [{}]\n",
        toml_string(&addon.id),
        toml_string(&addon.name),
        toml_string(&addon.version),
        toml_string(&addon.description),
        toml_string(&addon.author),
        toml_string(&addon.min_zem_version),
        platforms,
    );

    for pane in &addon.panes {
        toml.push_str(&format!(
            "\n[[pane]]\nname = \"{}\"\n",
            toml_string(&pane.name)
        ));
        if !pane.icon.is_empty() {
            toml.push_str(&format!("icon = \"{}\"\n", toml_string(&pane.icon)));
        }
        if let Some(collection) = &pane.collection {
            toml.push_str(&format!("collection = \"{}\"\n", toml_string(collection)));
        }
        serialize_operations(&pane.queries, "[[pane.query]]", &mut toml);
        serialize_operations(&pane.mutations, "[[pane.mutation]]", &mut toml);
    }

    for collection in &addon.collections {
        toml.push_str(&format!(
            "\n[[collection]]\nid = \"{}\"\n",
            toml_string(&collection.id)
        ));
        if !collection.name.is_empty() && collection.name != collection.id {
            toml.push_str(&format!("name = \"{}\"\n", toml_string(&collection.name)));
        }
    }

    for event in &addon.app_events {
        toml.push_str(&format!(
            "\n[[event]]\ntype = \"{}\"\n",
            toml_string(&event.event_type)
        ));
    }

    for action in &addon.actions {
        toml.push_str(&format!(
            "\n[[action]]\nid = \"{}\"\n",
            toml_string(&action.id)
        ));
        if action.handler.is_some() {
            toml.push_str("executable = true\n");
        }
        if !action.requires_write {
            toml.push_str("requires_write = false\n");
        }
        if !action.title.is_empty() {
            toml.push_str(&format!("title = \"{}\"\n", toml_string(&action.title)));
        }
        if !action.icon.is_empty() {
            toml.push_str(&format!("icon = \"{}\"\n", toml_string(&action.icon)));
        }
        if !action.description.is_empty() {
            toml.push_str(&format!(
                "description = \"{}\"\n",
                toml_string(&action.description)
            ));
        }
        if !action.location.is_empty() {
            toml.push_str(&format!(
                "location = \"{}\"\n",
                toml_string(&action.location)
            ));
        }
    }

    for field in &addon.config_fields {
        toml.push_str(&format!(
            "\n[[config]]\nname = \"{}\"\n",
            toml_string(&field.name)
        ));
        if !field.label.is_empty() && field.label != field.name {
            toml.push_str(&format!("label = \"{}\"\n", toml_string(&field.label)));
        }
        toml.push_str(&format!("type = \"{}\"\n", toml_string(&field.field_type)));
        if !field.default.is_empty() {
            toml.push_str(&format!("default = \"{}\"\n", toml_string(&field.default)));
        }
        if !field.description.is_empty() {
            toml.push_str(&format!(
                "description = \"{}\"\n",
                toml_string(&field.description)
            ));
        }
        for choice in &field.choices {
            toml.push_str(&format!(
                "\n[[config.choice]]\nvalue = \"{}\"\nlabel = \"{}\"\n",
                toml_string(&choice.value),
                toml_string(&choice.label)
            ));
        }
    }

    toml
}
