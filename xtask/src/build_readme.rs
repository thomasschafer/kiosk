use anyhow::{Context, Result};
use kiosk_core::config::{KeysConfig, NamedColor, ThemeConfig};
use quote::ToTokens;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::fs;
use std::path::Path;
use syn::{Attribute, Field, Fields, Item, ItemMod, ItemStruct, Meta, parse_file};

const CONFIG_START_MARKER: &str = "<!-- CONFIG START -->";
const CONFIG_END_MARKER: &str = "<!-- CONFIG END -->";

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

    let updated_content = generate_config_docs(&readme_content, config_path)?;

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
        process_struct(&mut docs, config_struct, &structs, "")?;
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
) -> Result<()> {
    if struct_item.ident == "ThemeConfig" && toml_prefix == "theme" {
        docs.push_str(&generate_theme_docs()?);
        return Ok(());
    }

    if struct_item.ident == "KeysConfig" && toml_prefix == "keys" {
        docs.push_str("Defaults are shown below.\n\n");
        docs.push_str("```toml\n");
        docs.push_str(&generate_default_keys_toml()?);
        docs.push_str("```\n\n");
        return Ok(());
    }

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

                    process_struct(docs, nested_struct, all_structs, &toml_path)?;
                } else {
                    let _ = writeln!(docs, "#### `{field_name}`\n");
                    docs.push_str(&field_doc);
                    docs.push_str("\n\n");
                }
            }
        }
    }

    Ok(())
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
    let mut structs = HashMap::new();
    let mut visited = HashSet::new();
    parse_structs_from_file(config_path, &mut structs, &mut visited)?;
    Ok(structs)
}

fn parse_structs_from_file(
    source_path: &Path,
    structs: &mut HashMap<String, ItemStruct>,
    visited: &mut HashSet<String>,
) -> Result<()> {
    let canonical = fs::canonicalize(source_path)
        .unwrap_or_else(|_| source_path.to_path_buf())
        .display()
        .to_string();
    if !visited.insert(canonical) {
        return Ok(());
    }

    let content = fs::read_to_string(source_path).context(format!(
        "Failed to read config file: {}",
        source_path.display()
    ))?;

    let syntax = parse_file(&content).context(format!(
        "Failed to parse config file: {}",
        source_path.display()
    ))?;

    for item in &syntax.items {
        match item {
            Item::Struct(s) => {
                structs.insert(s.ident.to_string(), s.clone());
            }
            Item::Mod(item_mod) => {
                if item_mod.content.is_none()
                    && let Some(module_path) = resolve_module_path(source_path, item_mod)
                {
                    parse_structs_from_file(&module_path, structs, visited)?;
                }
            }
            _ => {}
        }
    }

    Ok(())
}

fn resolve_module_path(source_path: &Path, item_mod: &ItemMod) -> Option<std::path::PathBuf> {
    let base_dir = source_path.parent()?;
    let module_name = item_mod.ident.to_string();
    let file_candidate = base_dir.join(format!("{module_name}.rs"));
    if file_candidate.exists() {
        return Some(file_candidate);
    }
    let mod_candidate = base_dir.join(module_name).join("mod.rs");
    if mod_candidate.exists() {
        return Some(mod_candidate);
    }
    None
}

fn generate_theme_docs() -> Result<String> {
    let mut docs = String::new();

    // List available colours and aliases (auto-generated from NamedColor)
    let color_names: Vec<&str> = NamedColor::all().iter().map(|(name, _)| *name).collect();
    let alias_notes: Vec<String> = NamedColor::aliases()
        .iter()
        .map(|(alias, canonical)| format!("`{alias}` for `{canonical}`"))
        .collect();
    let _ = writeln!(
        docs,
        "Colors can be a named color ({}) or a hex value (`#rrggbb`). Alternative spellings are also accepted: {}.\n",
        color_names
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", "),
        alias_notes.join(", "),
    );

    // Generate default TOML
    docs.push_str("Defaults:\n\n```toml\n");
    docs.push_str(&generate_default_theme_toml()?);
    docs.push_str("```\n\n");

    Ok(docs)
}

fn generate_default_theme_toml() -> Result<String> {
    #[derive(serde::Serialize)]
    struct ThemeWrapper<'a> {
        theme: &'a ThemeConfig,
    }

    let theme = ThemeConfig::default();
    let wrapped = ThemeWrapper { theme: &theme };
    let toml_str =
        toml::to_string_pretty(&wrapped).context("Failed to serialize default theme config")?;
    Ok(toml_str)
}

fn generate_default_keys_toml() -> Result<String> {
    #[derive(serde::Serialize)]
    struct KeysWrapper<'a> {
        keys: &'a KeysConfig,
    }

    let keys = KeysConfig::default();
    let wrapped = KeysWrapper { keys: &keys };
    let value =
        toml::Value::try_from(&wrapped).context("Failed to serialize default keys config")?;
    let section_table = value
        .get("keys")
        .and_then(toml::Value::as_table)
        .context("Serialized keys config is missing [keys] table")?;
    let mut out = String::new();
    let ordered_sections = KeysConfig::docs_section_order_asc();

    for section in ordered_sections {
        let section_value = section_table
            .get(section)
            .with_context(|| format!("Missing serialized key section: [keys.{section}]"))?;
        write_keymap_section(&mut out, section, section_value)?;
    }

    Ok(out)
}

fn write_keymap_section(
    out: &mut String,
    section: &str,
    section_value: &toml::Value,
) -> Result<()> {
    let _ = writeln!(out, "[keys.{section}]");
    let mut entries: Vec<_> = section_value
        .as_table()
        .with_context(|| format!("Expected [keys.{section}] to serialize as a TOML table"))?
        .iter()
        .map(|(key, command)| {
            command
                .as_str()
                .with_context(|| {
                    format!("Expected [keys.{section}] \"{key}\" value to serialize as string")
                })
                .map(|value| (key.clone(), value.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;
    entries.sort_unstable();
    for (key, command) in entries {
        let _ = writeln!(
            out,
            "\"{}\" = \"{}\"",
            escape_toml_string(&key),
            escape_toml_string(&command)
        );
    }
    out.push('\n');
    Ok(())
}

fn escape_toml_string(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
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
