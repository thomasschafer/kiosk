use anyhow::{Context, Result};
use kiosk_core::config::keys::{Command, KeyMap, KeysConfig};
use quote::ToTokens;
use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
use std::path::Path;
use syn::{Attribute, Field, Fields, Item, ItemStruct, Meta, parse_file};

const CONFIG_START_MARKER: &str = "<!-- CONFIG START -->";
const CONFIG_END_MARKER: &str = "<!-- CONFIG END -->";
const KEYS_START_MARKER: &str = "<!-- KEYS START -->";
const KEYS_END_MARKER: &str = "<!-- KEYS END -->";

pub fn generate_readme(readme_path: &Path, config_path: &Path, check_only: bool) -> Result<()> {
    println!("Processing README file: {}", readme_path.display());

    let readme_content = fs::read_to_string(readme_path).context(format!(
        "Failed to read README file: {}",
        readme_path.display()
    ))?;

    if !config_path.exists() {
        println!("Error: Config file '{}' not found.", config_path.display());
        std::process::exit(1);
    }

    let mut updated_content = generate_config_docs(&readme_content, config_path)?;
    updated_content = generate_keybindings_docs(&updated_content);

    if updated_content == readme_content {
        println!("README file is already up to date");
    } else {
        if check_only {
            println!(
                "README is out of date and needs regenerating. Run: cargo run -p xtask -- readme"
            );
            std::process::exit(1);
        }
        fs::write(readme_path, &updated_content).context(format!(
            "Failed to write README file: {}",
            readme_path.display()
        ))?;

        println!("README file updated successfully");
    }

    Ok(())
}

fn generate_config_docs(content: &str, config_path: &Path) -> Result<String> {
    println!("Extracting config documentation...");

    let structs = parse_config_structs(config_path)?;

    let mut docs = String::new();

    if let Some(config_struct) = structs.get("Config") {
        process_struct(&mut docs, config_struct, &structs, "");
    } else {
        anyhow::bail!("Config struct not found in the source file.");
    }

    Ok(replace_section_content(
        content,
        CONFIG_START_MARKER,
        CONFIG_END_MARKER,
        &docs,
    ))
}

fn process_struct(
    docs: &mut String,
    struct_item: &ItemStruct,
    all_structs: &HashMap<String, ItemStruct>,
    toml_prefix: &str,
) {
    if let Fields::Named(ref fields) = struct_item.fields {
        for field in &fields.named {
            if let Some(ident) = &field.ident {
                let field_name = ident.to_string();
                let field_doc = extract_doc_comment(&field.attrs);

                if let Some(nested_struct) = all_structs.get(&get_type_name(field)) {
                    let toml_path = if toml_prefix.is_empty() {
                        field_name.clone()
                    } else {
                        format!("{toml_prefix}.{field_name}")
                    };

                    #[allow(clippy::format_push_string)]
                    docs.push_str(&format!("### `[{toml_path}]` section\n\n"));

                    if !field_doc.is_empty() {
                        docs.push_str(&field_doc);
                        docs.push_str("\n\n");
                    }

                    process_struct(docs, nested_struct, all_structs, &toml_path);
                } else {
                    let _ = writeln!(docs, "#### `{field_name}`\n");
                    docs.push_str(&field_doc);
                    docs.push_str("\n\n");
                }
            }
        }
    }
}

fn get_type_name(field: &Field) -> String {
    field
        .ty
        .to_token_stream()
        .to_string()
        .trim_start_matches("Option < ")
        .trim_end_matches(" >")
        .to_string()
}

fn extract_doc_comment(attrs: &[Attribute]) -> String {
    let mut doc_lines = Vec::new();

    for attr in attrs {
        if attr.path().is_ident("doc")
            && let Meta::NameValue(meta) = attr.meta.clone()
            && let syn::Expr::Lit(expr_lit) = meta.value
            && let syn::Lit::Str(lit_str) = expr_lit.lit
        {
            let comment = lit_str.value();
            doc_lines.push(comment.trim().to_string());
        }
    }

    doc_lines.join("\n")
}

fn parse_config_structs(config_path: &Path) -> Result<HashMap<String, ItemStruct>> {
    let content = fs::read_to_string(config_path).context(format!(
        "Failed to read config file: {}",
        config_path.display()
    ))?;

    let syntax = parse_file(&content).context(format!(
        "Failed to parse config file: {}",
        config_path.display()
    ))?;

    let mut structs = HashMap::new();
    for item in &syntax.items {
        if let Item::Struct(s) = item {
            structs.insert(s.ident.to_string(), s.clone());
        }
    }

    Ok(structs)
}

fn generate_keybindings_docs(content: &str) -> String {
    println!("Generating keybindings documentation...");

    let keys_config = KeysConfig::default();
    let mut docs = String::new();

    // Generate tables for each mode
    generate_keymap_table(&mut docs, "General", &keys_config.general);
    generate_keymap_table(&mut docs, "Repository Selection", &keys_config.repo_select);
    generate_keymap_table(&mut docs, "Branch Selection", &keys_config.branch_select);
    generate_keymap_table(
        &mut docs,
        "New Branch Base Selection",
        &keys_config.new_branch_base,
    );
    generate_keymap_table(&mut docs, "Confirmation", &keys_config.confirmation);

    // Add note about search functionality
    docs.push_str("### Search\n\n");
    docs.push_str("In list modes (Repository, Branch, and New Branch Base Selection), any printable character will start or continue search filtering.\n");

    if !content.contains(KEYS_START_MARKER) || !content.contains(KEYS_END_MARKER) {
        eprintln!(
            "Error: keybindings markers ({KEYS_START_MARKER} / {KEYS_END_MARKER}) not found in README"
        );
        std::process::exit(1);
    }

    replace_section_content(content, KEYS_START_MARKER, KEYS_END_MARKER, &docs)
}

fn generate_keymap_table(docs: &mut String, mode_name: &str, keymap: &KeyMap) {
    if keymap.is_empty() {
        return;
    }

    let _ = write!(docs, "### {mode_name}\n\n");
    docs.push_str("| Key | Action |\n");
    docs.push_str("|-----|--------|\n");

    // Convert to vector and sort by key display for consistent ordering
    let mut bindings: Vec<_> = keymap.iter().collect();
    bindings.sort_by_key(|(key, _)| key.to_string());

    for (key_event, command) in bindings {
        if *command == Command::Noop {
            continue; // Skip unbound keys
        }
        let key_str = key_event.to_string().replace('|', "\\|");
        let description = command.description().replace('|', "\\|");
        let _ = writeln!(docs, "| {key_str} | {description} |");
    }

    docs.push('\n');
}

fn replace_section_content(
    content: &str,
    start_marker: &str,
    end_marker: &str,
    new_content: &str,
) -> String {
    let mut result = String::new();
    let mut in_section = false;

    for line in content.lines() {
        if line == start_marker {
            result.push_str(line);
            result.push('\n');
            result.push_str(new_content);
            in_section = true;
        } else if line == end_marker {
            in_section = false;
            result.push_str(line);
            result.push('\n');
        } else if !in_section {
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}
