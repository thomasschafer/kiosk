//! Generates the configuration reference in a kiosk product's README from the doc comments
//! and defaults of its config types: kiosk-core's shared sections and the product's own
//! [`ConfigExtension`]. Every documented type needs a user-facing doc comment, so the
//! reference cannot silently fall behind the code.

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    fs,
    path::Path,
};

use anyhow::{Context, Result, bail};
use kiosk_core::config::{ConfigExtension, CoreConfig, keys::default_keys_toml};
use syn::{
    Attribute, Field, Fields, GenericArgument, Item, ItemEnum, ItemStruct, Meta, PathArguments,
    Type, parse_file,
};

const CONFIG_START: &str = "<!-- CONFIG:START -->";
const CONFIG_END: &str = "<!-- CONFIG:END -->";
const CORE_ROOT: &str = "CoreConfig";

/// What a product adds to the shared reference.
pub struct ProductDocs<'a> {
    /// The Rust source of the product's config types, including its extension.
    pub sources: &'a [&'a str],
    /// The product's extension struct. Its doc comment introduces the reference, so it
    /// says where the product reads its config from.
    pub extension: &'a str,
    /// Every top-level section, core and product alike, in the order to document them.
    pub order: &'a [&'a str],
    /// TOML examples shown with the product's own sections, by section name.
    pub examples: &'a [(&'a str, &'a str)],
}

/// Regenerates the marked config section of the README at `readme_path`, or with `check`
/// only fails if it is out of date.
pub fn generate<E: ConfigExtension>(
    readme_path: &Path,
    docs: &ProductDocs<'_>,
    check: bool,
) -> Result<()> {
    let raw = fs::read_to_string(readme_path)
        .with_context(|| format!("failed to read {}", readme_path.display()))?;
    let current = normalize_line_endings(&raw);
    let generated = replace_generated_section(&current, &config_reference::<E>(docs)?)?;

    if generated == current && (check || generated == raw) {
        println!("{} is up to date", readme_path.display());
        return Ok(());
    }
    if check {
        bail!(
            "{} is out of date; run `cargo xtask readme`",
            readme_path.display()
        );
    }

    fs::write(readme_path, generated)
        .with_context(|| format!("failed to write {}", readme_path.display()))?;
    println!("updated {}", readme_path.display());
    Ok(())
}

fn normalize_line_endings(input: &str) -> String {
    input.replace('\r', "")
}

/// The Markdown reference for the core sections and `E`'s.
pub fn config_reference<E: ConfigExtension>(docs: &ProductDocs<'_>) -> Result<String> {
    let model = ConfigModel::parse(kiosk_core::config::SOURCES.iter().chain(docs.sources))?;
    let core = model
        .structs
        .get(CORE_ROOT)
        .context("CoreConfig not found in kiosk-core's config sources")?;
    let extension = model.structs.get(docs.extension).with_context(|| {
        format!(
            "{} not found in the product's config sources",
            docs.extension
        )
    })?;
    let mut defaults = toml::Value::try_from(CoreConfig::default())
        .context("failed to serialize CoreConfig::default() as TOML")?;
    let extension_defaults = toml::Value::try_from(E::default())
        .with_context(|| format!("failed to serialize the default {} as TOML", docs.extension))?;
    merge_tables(&mut defaults, extension_defaults)?;

    let mut fields: HashMap<String, &Field> = HashMap::new();
    for root in [core, extension] {
        for field in named_fields(root)? {
            if fields.insert(field_name(field)?, field).is_some() {
                bail!(
                    "{} redeclares the core section `{}`",
                    docs.extension,
                    field_name(field)?
                );
            }
        }
    }
    let ordered: HashSet<&str> = docs.order.iter().copied().collect();
    if ordered.len() != docs.order.len() {
        bail!("the section order lists a section twice");
    }
    if let Some(missing) = fields.keys().find(|name| !ordered.contains(name.as_str())) {
        bail!("the section order leaves out `{missing}`");
    }

    let mut output = String::new();
    push_doc(
        &mut output,
        &required_doc(&extension.attrs, docs.extension)?,
    );
    for name in docs.order {
        let field = fields
            .get(*name)
            .with_context(|| format!("the section order names unknown section `{name}`"))?;
        render_config_field(&mut output, field, &model, &defaults, docs.examples)?;
    }
    Ok(output)
}

fn merge_tables(into: &mut toml::Value, from: toml::Value) -> Result<()> {
    let (toml::Value::Table(into), toml::Value::Table(from)) = (into, from) else {
        bail!("config defaults must serialize as TOML tables");
    };
    for (key, value) in from {
        if into.insert(key.clone(), value).is_some() {
            bail!("the extension's default redeclares the core section `{key}`");
        }
    }
    Ok(())
}

fn render_config_field(
    docs: &mut String,
    field: &Field,
    model: &ConfigModel,
    defaults: &toml::Value,
    examples: &[(&str, &str)],
) -> Result<()> {
    let name = field_name(field)?;
    let field_doc = required_doc(&field.attrs, &format!("config field {name}"))?;
    let default = defaults.get(&name);

    if let Some(type_name) = direct_type_name(&field.ty)
        && let Some(nested) = model.structs.get(&type_name)
    {
        writeln!(docs, "### `[{name}]`\n")?;
        push_doc(docs, &field_doc);

        match type_name.as_str() {
            "ThemeConfig" => render_theme(docs, nested, model, default)?,
            "KeysConfig" => render_keys(docs),
            _ => {
                push_distinct_doc(docs, &field_doc, &required_doc(&nested.attrs, &type_name)?);
                render_curated_example(docs, &name, examples);
                render_struct_fields(docs, nested, model, default)?;
            }
        }
        return Ok(());
    }

    writeln!(docs, "#### `{name}`\n")?;
    push_doc(docs, &field_doc);
    render_curated_example(docs, &name, examples);
    render_collection_schema(docs, field, model)?;
    render_default(docs, default)?;
    Ok(())
}

fn render_curated_example(docs: &mut String, name: &str, examples: &[(&str, &str)]) {
    let example = match name {
        "search_dirs" => Some(
            "include_non_git = false\nsearch_dirs = [\n  \"~/Code\",\n  { path = \"~/Work\", depth = 2, include_non_git = true },\n  { path = \"~/Clients\", optional = true },\n]",
        ),
        _ => examples
            .iter()
            .find(|(section, _)| *section == name)
            .map(|(_, example)| *example),
    };
    if let Some(example) = example {
        docs.push_str("Example:\n\n```toml\n");
        docs.push_str(example.trim_end());
        docs.push_str("\n```\n\n");
    }
}

fn render_struct_fields(
    docs: &mut String,
    item: &ItemStruct,
    model: &ConfigModel,
    defaults: Option<&toml::Value>,
) -> Result<()> {
    for field in named_fields(item)? {
        let name = field_name(field)?;
        writeln!(docs, "#### `{name}`\n")?;
        push_doc(
            docs,
            &required_doc(&field.attrs, &format!("{}::{name}", item.ident))?,
        );
        render_collection_schema(docs, field, model)?;
        render_default(docs, defaults.and_then(|value| value.get(&name)))?;
    }
    Ok(())
}

fn render_collection_schema(docs: &mut String, field: &Field, model: &ConfigModel) -> Result<()> {
    let Some(element_name) = collection_element_name(&field.ty) else {
        return Ok(());
    };

    if let Some(item) = model.structs.get(&element_name) {
        push_doc(docs, &required_doc(&item.attrs, &element_name)?);
        docs.push_str("Each entry is an inline table with:\n\n");
        for nested in named_fields(item)? {
            let name = field_name(nested)?;
            let prose = required_list_item_doc(&nested.attrs, &format!("{element_name}::{name}"))?;
            writeln!(docs, "- `{name}` — {prose}")?;
        }
        docs.push('\n');
    } else if let Some(item) = model.enums.get(&element_name) {
        push_doc(docs, &required_doc(&item.attrs, &element_name)?);
        docs.push_str("Accepted forms:\n\n");
        for variant in &item.variants {
            let variant_doc = required_list_item_doc(
                &variant.attrs,
                &format!("{element_name}::{}", variant.ident),
            )?;
            match &variant.fields {
                Fields::Unnamed(fields) => {
                    writeln!(docs, "- {variant_doc}")?;
                    for (index, nested) in fields.unnamed.iter().enumerate() {
                        let prose = required_list_item_doc(
                            &nested.attrs,
                            &format!("{element_name}::{} field {index}", variant.ident),
                        )?;
                        writeln!(docs, "  - {prose}")?;
                    }
                }
                Fields::Unit => {
                    writeln!(docs, "- {variant_doc}")?;
                }
                Fields::Named(fields) => {
                    writeln!(docs, "- {variant_doc}")?;
                    for nested in &fields.named {
                        let name = field_name(nested)?;
                        let prose = required_list_item_doc(
                            &nested.attrs,
                            &format!("{element_name}::{}::{name}", variant.ident),
                        )?;
                        writeln!(docs, "  - `{name}` — {prose}")?;
                    }
                }
            }
        }
        docs.push('\n');
    }
    Ok(())
}

fn render_keys(docs: &mut String) {
    docs.push_str("Assign a key to `\"noop\"` to unbind an inherited mapping.\n\n");
    docs.push_str(
        "Write chords with lowercase `ctrl+`, `alt+`, and `shift+` modifiers followed by a character or a named key such as `enter`, `esc`, `tab`, `backspace`, `delete`, an arrow, `home`, `end`, `pageup`, `pagedown`, or `space`.\n\n",
    );
    docs.push_str("Defaults:\n\n");
    docs.push_str(&default_keys_toml());
    docs.push('\n');
}

fn render_theme(
    docs: &mut String,
    theme: &ItemStruct,
    model: &ConfigModel,
    default: Option<&toml::Value>,
) -> Result<()> {
    let theme_color = model
        .enums
        .get("ThemeColor")
        .context("ThemeColor enum not found in the config module")?;
    let colors = theme_color
        .variants
        .iter()
        .map(|variant| format!("`{}`", snake_case(&variant.ident.to_string())))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        docs,
        "Colors use the terminal's ANSI palette and can be {colors}.\n"
    )?;
    docs.push_str("Defaults:\n\n```toml\n[theme]\n");
    for field in named_fields(theme)? {
        let name = field_name(field)?;
        let value = default
            .and_then(|value| value.get(&name))
            .with_context(|| format!("ThemeConfig::{name} is missing from Config::default()"))?;
        writeln!(docs, "{name} = {}", format_toml_value(value))?;
    }
    docs.push_str("```\n\n");
    Ok(())
}

fn render_default(docs: &mut String, default: Option<&toml::Value>) -> Result<()> {
    if let Some(value) = default.filter(|value| !is_empty_composite(value)) {
        writeln!(docs, "Default: `{}`\n", format_toml_value(value))?;
    }
    Ok(())
}

fn named_fields(
    item: &ItemStruct,
) -> Result<&syn::punctuated::Punctuated<Field, syn::token::Comma>> {
    match &item.fields {
        Fields::Named(fields) => Ok(&fields.named),
        _ => bail!("{} must use named fields", item.ident),
    }
}

fn field_name(field: &Field) -> Result<String> {
    field
        .ident
        .as_ref()
        .map(ToString::to_string)
        .context("expected a named field")
}

fn direct_type_name(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else {
        return None;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

fn collection_element_name(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != "Vec" {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let GenericArgument::Type(Type::Path(element)) = arguments.args.first()? else {
        return None;
    };
    element
        .path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

fn extract_doc(attrs: &[Attribute]) -> String {
    attrs
        .iter()
        .filter_map(|attribute| {
            if !attribute.path().is_ident("doc") {
                return None;
            }
            let Meta::NameValue(meta) = &attribute.meta else {
                return None;
            };
            let syn::Expr::Lit(expression) = &meta.value else {
                return None;
            };
            let syn::Lit::Str(value) = &expression.lit else {
                return None;
            };
            Some(value.value().trim().to_owned())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn required_doc(attrs: &[Attribute], item: &str) -> Result<String> {
    let doc = extract_doc(attrs);
    if doc.is_empty() {
        bail!("missing user-facing doc comment on {item}");
    }
    Ok(doc)
}

/// Joins wrapped doc-comment lines so the prose stays within one Markdown list item.
fn required_list_item_doc(attrs: &[Attribute], item: &str) -> Result<String> {
    required_doc(attrs, item).map(|doc| doc.replace('\n', " "))
}

fn push_doc(output: &mut String, doc: &str) {
    output.push_str(doc);
    output.push_str("\n\n");
}

fn push_distinct_doc(output: &mut String, first: &str, second: &str) {
    if first != second {
        push_doc(output, second);
    }
}

fn format_toml_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(value) => format!("\"{}\"", value.replace('"', "\\\"")),
        other => other.to_string(),
    }
}

fn is_empty_composite(value: &toml::Value) -> bool {
    match value {
        toml::Value::Array(values) => values.is_empty(),
        toml::Value::Table(values) => values.is_empty(),
        _ => false,
    }
}

fn snake_case(name: &str) -> String {
    let mut output = String::new();
    for (index, character) in name.chars().enumerate() {
        if character.is_ascii_uppercase() && index > 0 {
            output.push('_');
        }
        output.push(character.to_ascii_lowercase());
    }
    output
}

fn replace_generated_section(readme: &str, docs: &str) -> Result<String> {
    let (before, remainder) = readme
        .split_once(CONFIG_START)
        .context("README is missing the config start marker")?;
    if before.contains(CONFIG_END) || remainder.matches(CONFIG_START).count() != 0 {
        bail!("README config markers are duplicated or out of order");
    }
    let (_, after) = remainder
        .split_once(CONFIG_END)
        .context("README is missing the config end marker")?;
    if after.contains(CONFIG_END) {
        bail!("README config markers are duplicated");
    }
    Ok(format!("{before}{CONFIG_START}\n{docs}{CONFIG_END}{after}"))
}

#[derive(Default)]
struct ConfigModel {
    structs: HashMap<String, ItemStruct>,
    enums: HashMap<String, ItemEnum>,
}

impl ConfigModel {
    fn parse<'a>(sources: impl IntoIterator<Item = &'a &'a str>) -> Result<Self> {
        let mut model = Self::default();
        for source in sources {
            let syntax = parse_file(source).context("failed to parse config source")?;
            for item in syntax.items {
                match item {
                    Item::Struct(item) => {
                        model.structs.insert(item.ident.to_string(), item);
                    }
                    Item::Enum(item) => {
                        model.enums.insert(item.ident.to_string(), item);
                    }
                    _ => {}
                }
            }
        }
        Ok(model)
    }
}

#[cfg(test)]
mod tests {
    use kiosk_core::config::NoExtension;
    use serde::{Deserialize, Serialize};

    use super::*;

    const EXTENSION: &str = r"
/// Read from the test product's config file.
pub struct TestExtension {
    /// Lines of the test section.
    pub extra: Vec<String>,
}
";

    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    struct TestExtension {
        extra: Vec<String>,
    }

    impl ConfigExtension for TestExtension {
        fn validate(&self) -> Result<()> {
            Ok(())
        }
    }

    fn docs<'a>(order: &'a [&'a str]) -> ProductDocs<'a> {
        ProductDocs {
            sources: &[EXTENSION],
            extension: "TestExtension",
            order,
            examples: &[("extra", "extra = [\"a\"]")],
        }
    }

    const ORDER: &[&str] = &["search_dirs", "include_non_git", "extra", "theme", "keys"];

    #[test]
    fn the_reference_follows_the_given_order_with_product_examples() {
        let reference = config_reference::<TestExtension>(&docs(ORDER)).unwrap();
        assert!(reference.starts_with("Read from the test product's config file."));
        let position = |needle: &str| reference.find(needle).unwrap();
        assert!(position("#### `search_dirs`") < position("#### `extra`"));
        assert!(position("#### `extra`") < position("### `[theme]`"));
        assert!(position("### `[theme]`") < position("### `[keys]`"));
        assert!(reference.contains("```toml\nextra = [\"a\"]\n```"));
    }

    #[test]
    fn every_section_must_be_placed_exactly_once() {
        let missing = config_reference::<TestExtension>(&docs(&["search_dirs"])).unwrap_err();
        assert!(missing.to_string().contains("leaves out"), "{missing}");
        let unknown =
            config_reference::<TestExtension>(&docs(&[ORDER, &["nope"]].concat())).unwrap_err();
        assert!(unknown.to_string().contains("unknown section"), "{unknown}");
    }

    #[test]
    fn a_product_without_an_extension_documents_the_core_sections() {
        let sources: &[&str] =
            &["/// Read from the bare product's config file.\npub struct NoExtension {}"];
        let reference = config_reference::<NoExtension>(&ProductDocs {
            sources,
            extension: "NoExtension",
            order: &["search_dirs", "include_non_git", "theme", "keys"],
            examples: &[],
        })
        .unwrap();
        assert!(reference.contains("### `[keys]`"));
    }

    #[test]
    fn replaces_only_the_marked_section() {
        let input = "before\n<!-- CONFIG:START -->\nstale\n<!-- CONFIG:END -->\nafter\n";
        assert_eq!(
            replace_generated_section(input, "fresh\n").unwrap(),
            "before\n<!-- CONFIG:START -->\nfresh\n<!-- CONFIG:END -->\nafter\n"
        );
    }

    #[test]
    fn formats_rust_names_as_serde_snake_case() {
        assert_eq!(snake_case("DarkGray"), "dark_gray");
    }

    #[test]
    fn normalizes_crlf_line_endings() {
        assert_eq!(
            normalize_line_endings("first\r\nsecond\r\n"),
            "first\nsecond\n"
        );
    }
}
