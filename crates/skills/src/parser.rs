//! SKILL.md parsing per L3-BEH-SKILLS-001 §3.

use std::path::Path;

use crate::package::{
    FrontmatterFormat, SkillDefinition, SkillDiagnostic, SkillName, SkillPackage, SkillPackageId,
    SkillSourceKind,
};

/// Parse a SKILL.md file at the given path into a SkillDefinition.
///
/// Parsing rules:
/// 1. Read the file as UTF-8.
/// 2. Extract `---` delimited frontmatter block.
/// 3. Parse YAML or TOML-like frontmatter.
/// 4. Require `name` and `description`.
/// 5. Treat content after frontmatter as `instruction_body`.
/// 6. Bound sizes: frontmatter ≤ 32KB, body ≤ 256KB, total ≤ 256KB.
pub fn parse_skill_md(path: &Path) -> Result<SkillPackage, Vec<SkillDiagnostic>> {
    let mut diagnostics = Vec::new();

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            return Err(vec![SkillDiagnostic::UnreadableEntrypoint {
                reason: e.to_string(),
            }]);
        }
    };

    let (frontmatter_raw, body_raw, frontmatter_format) = extract_frontmatter(&content);

    // Size bounds
    const MAX_FRONTMATTER: usize = 32768; // 32KB
    const MAX_BODY: usize = 262144; // 256KB
    const MAX_TOTAL: usize = 262144; // 256KB

    if frontmatter_raw.len() > MAX_FRONTMATTER {
        diagnostics.push(SkillDiagnostic::InvalidFrontmatter {
            reason: format!(
                "frontmatter too large: {} bytes (max {})",
                frontmatter_raw.len(),
                MAX_FRONTMATTER
            ),
        });
        return Err(diagnostics);
    }

    if body_raw.len() > MAX_BODY {
        diagnostics.push(SkillDiagnostic::BodyTooLarge {
            bytes: body_raw.len(),
        });
        return Err(diagnostics);
    }

    if content.len() > MAX_TOTAL {
        diagnostics.push(SkillDiagnostic::BodyTooLarge {
            bytes: content.len(),
        });
        return Err(diagnostics);
    }

    // Parse frontmatter
    let name: Option<String>;
    let description: Option<String>;
    let version: Option<String>;
    let tags: Vec<String>;
    let allowed_tools: Vec<String>;

    match frontmatter_format {
        FrontmatterFormat::Yaml => match parse_yaml_frontmatter(frontmatter_raw) {
            Ok(fields) => {
                name = fields.name;
                description = fields.description;
                version = fields.version;
                tags = fields.tags;
                allowed_tools = fields.allowed_tools;
            }
            Err(reason) => {
                diagnostics.push(SkillDiagnostic::InvalidFrontmatter { reason });
                return Err(diagnostics);
            }
        },
        FrontmatterFormat::Toml => match parse_toml_frontmatter(frontmatter_raw) {
            Ok(fields) => {
                name = fields.name;
                description = fields.description;
                version = fields.version;
                tags = fields.tags;
                allowed_tools = fields.allowed_tools;
            }
            Err(reason) => {
                diagnostics.push(SkillDiagnostic::InvalidFrontmatter { reason });
                return Err(diagnostics);
            }
        },
        FrontmatterFormat::Unknown => {
            diagnostics.push(SkillDiagnostic::InvalidFrontmatter {
                reason: "could not identify frontmatter format".into(),
            });
            return Err(diagnostics);
        }
    }

    // Validate required fields
    let name_str = match name {
        Some(n) => n,
        None => {
            diagnostics.push(SkillDiagnostic::MissingRequiredField {
                field: "name".to_string(),
            });
            return Err(diagnostics);
        }
    };

    let description = match description {
        Some(d) => d,
        None => {
            diagnostics.push(SkillDiagnostic::MissingRequiredField {
                field: "description".to_string(),
            });
            return Err(diagnostics);
        }
    };

    if description.len() > 1000 {
        diagnostics.push(SkillDiagnostic::DescriptionTooLong {
            bytes: description.len(),
        });
    }

    let skill_name = match SkillName::new(&name_str) {
        Ok(n) => n,
        Err(_) => {
            diagnostics.push(SkillDiagnostic::InvalidName { value: name_str });
            return Err(diagnostics);
        }
    };

    let root = path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    let definition = SkillDefinition {
        name: skill_name.clone(),
        description,
        version,
        enabled: Some(true),
        tags,
        compatibility: None,
        allowed_tools,
        instruction_body: body_raw.to_string(),
        frontmatter_format,
    };

    // Simple content-based hash using std's hasher
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    let content_hash = crate::package::Sha256Digest(format!("{:016x}", hasher.finish()));

    Ok(SkillPackage {
        package_id: SkillPackageId::new(SkillSourceKind::User, skill_name.clone(), root.clone()),
        root,
        definition,
        resources: vec![],
        content_hash,
        diagnostics,
    })
}

#[derive(Default)]
struct FrontmatterFields {
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    tags: Vec<String>,
    allowed_tools: Vec<String>,
}

/// Extract frontmatter and body from SKILL.md content.
fn extract_frontmatter(content: &str) -> (&str, &str, FrontmatterFormat) {
    let trimmed = content.trim();
    if !trimmed.starts_with("---") {
        return (trimmed, "", FrontmatterFormat::Unknown);
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    if let Some(end) = after_first.find("\n---") {
        let frontmatter = after_first[..end].trim();
        let body = after_first[end + 4..].trim();

        let format = detect_frontmatter_format(frontmatter);
        (frontmatter, body, format)
    } else if let Some(end) = after_first.find("---") {
        let frontmatter = after_first[..end].trim();
        let body = after_first[end + 3..].trim();

        let format = detect_frontmatter_format(frontmatter);
        (frontmatter, body, format)
    } else {
        (trimmed, "", FrontmatterFormat::Unknown)
    }
}

fn detect_frontmatter_format(frontmatter: &str) -> FrontmatterFormat {
    // Simple heuristic: if it starts with [ and contains ] =, it's TOML-like
    let trimmed = frontmatter.trim();
    if trimmed.contains('=') && !trimmed.contains(':') {
        return FrontmatterFormat::Toml;
    }
    if trimmed.contains(':') {
        return FrontmatterFormat::Yaml;
    }
    FrontmatterFormat::Unknown
}

fn parse_yaml_frontmatter(raw: &str) -> Result<FrontmatterFields, String> {
    let mut fields = FrontmatterFields::default();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            assign_frontmatter_field(&mut fields, key.trim(), value);
        }
    }

    if fields.name.is_none() && fields.description.is_none() {
        return Err("no recognizable fields found".into());
    }

    Ok(fields)
}

fn parse_toml_frontmatter(raw: &str) -> Result<FrontmatterFields, String> {
    // Simple TOML-like key=value parsing
    let mut fields = FrontmatterFields::default();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            assign_frontmatter_field(&mut fields, key.trim(), value);
        }
    }

    if fields.name.is_none() && fields.description.is_none() {
        return Err("no recognizable fields found".into());
    }

    Ok(fields)
}

fn assign_frontmatter_field(fields: &mut FrontmatterFields, key: &str, value: &str) {
    if key.eq_ignore_ascii_case("name") {
        fields.name = Some(value.to_string());
    } else if key.eq_ignore_ascii_case("description") {
        fields.description = Some(value.to_string());
    } else if key.eq_ignore_ascii_case("version") {
        fields.version = Some(value.to_string());
    } else if key.eq_ignore_ascii_case("tags") {
        fields.tags = parse_inline_string_list(value);
    } else if key.eq_ignore_ascii_case("allowed_tools") || key.eq_ignore_ascii_case("allowed-tools")
    {
        fields.allowed_tools = parse_inline_string_list(value);
    }
}

fn parse_inline_string_list(value: &str) -> Vec<String> {
    value
        .trim_matches('[')
        .trim_matches(']')
        .split(',')
        .filter_map(|item| {
            let item = item.trim().trim_matches('"').trim_matches('\'');
            (!item.is_empty()).then(|| item.to_string())
        })
        .collect()
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_skill_md(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut f = std::fs::File::create(&path).expect("create");
        f.write_all(content.as_bytes()).expect("write");
    }

    #[test]
    fn valid_skill_md_parses() {
        let tmp = TempDir::new().expect("tempdir");
        let pkg_dir = tmp.path().join("my-skill");
        std::fs::create_dir(&pkg_dir).expect("create");
        let skill_md = pkg_dir.join("SKILL.md");

        write_skill_md(
            &pkg_dir,
            "SKILL.md",
            "---\nname: my-skill\ndescription: A test skill\nversion: \"1.0\"\ntags: [test, example]\nallowed_tools: [read, write]\n---\n\nThis is the instruction body.\nIt has multiple lines.\n",
        );

        let result = parse_skill_md(&skill_md).expect("parse");
        assert_eq!(result.definition.name.as_str(), "my-skill");
        assert_eq!(result.definition.description, "A test skill");
        assert_eq!(result.definition.version.as_deref(), Some("1.0"));
        assert_eq!(result.definition.tags, vec!["test", "example"]);
        assert_eq!(result.definition.allowed_tools, vec!["read", "write"]);
        assert!(
            result
                .definition
                .instruction_body
                .contains("instruction body")
        );
        assert_eq!(
            result.definition.frontmatter_format,
            FrontmatterFormat::Yaml
        );
    }

    #[test]
    fn missing_name_produces_diagnostics() {
        let tmp = TempDir::new().expect("tempdir");
        let pkg_dir = tmp.path().join("bad-skill");
        std::fs::create_dir(&pkg_dir).expect("create");
        let skill_md = pkg_dir.join("SKILL.md");

        write_skill_md(
            &pkg_dir,
            "SKILL.md",
            "---\ndescription: No name here\n---\n\nbody",
        );

        let result = parse_skill_md(&skill_md);
        assert!(result.is_err());
        let diags = result.unwrap_err();
        assert!(diags.iter().any(
            |d| matches!(d, SkillDiagnostic::MissingRequiredField { field } if field == "name")
        ));
    }

    #[test]
    fn missing_description_produces_diagnostics() {
        let tmp = TempDir::new().expect("tempdir");
        let pkg_dir = tmp.path().join("bad-skill2");
        std::fs::create_dir(&pkg_dir).expect("create");
        let skill_md = pkg_dir.join("SKILL.md");

        write_skill_md(&pkg_dir, "SKILL.md", "---\nname: test\n---\n\nbody");

        let result = parse_skill_md(&skill_md);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_name_produces_diagnostics() {
        let tmp = TempDir::new().expect("tempdir");
        let pkg_dir = tmp.path().join("bad-name");
        std::fs::create_dir(&pkg_dir).expect("create");
        let skill_md = pkg_dir.join("SKILL.md");

        write_skill_md(
            &pkg_dir,
            "SKILL.md",
            "---\nname: bad name!\ndescription: test\n---\n\nbody",
        );

        let result = parse_skill_md(&skill_md);
        assert!(result.is_err());
        let diags = result.unwrap_err();
        assert!(
            diags
                .iter()
                .any(|d| matches!(d, SkillDiagnostic::InvalidName { .. }))
        );
    }

    #[test]
    fn body_too_large_produces_diagnostics() {
        let tmp = TempDir::new().expect("tempdir");
        let pkg_dir = tmp.path().join("large-skill");
        std::fs::create_dir(&pkg_dir).expect("create");
        let skill_md = pkg_dir.join("SKILL.md");

        let large_body = "x".repeat(300000);
        write_skill_md(
            &pkg_dir,
            "SKILL.md",
            &format!("---\nname: big\ndescription: huge\n---\n\n{}", large_body),
        );

        let result = parse_skill_md(&skill_md);
        assert!(result.is_err());
    }

    #[test]
    fn toml_like_frontmatter_parses() {
        let tmp = TempDir::new().expect("tempdir");
        let pkg_dir = tmp.path().join("toml-skill");
        std::fs::create_dir(&pkg_dir).expect("create");
        let skill_md = pkg_dir.join("SKILL.md");

        write_skill_md(
            &pkg_dir,
            "SKILL.md",
            "---\nname = \"toml-skill\"\ndescription = \"TOML style\"\n---\n\nbody",
        );

        let result = parse_skill_md(&skill_md).expect("parse");
        assert_eq!(result.definition.name.as_str(), "toml-skill");
        assert_eq!(
            result.definition.frontmatter_format,
            FrontmatterFormat::Toml
        );
    }

    #[test]
    fn frontmatter_keys_are_case_insensitive_and_lists_skip_empty_items() {
        let tmp = TempDir::new().expect("tempdir");
        let pkg_dir = tmp.path().join("case-skill");
        std::fs::create_dir(&pkg_dir).expect("create");
        let skill_md = pkg_dir.join("SKILL.md");

        write_skill_md(
            &pkg_dir,
            "SKILL.md",
            "---\nNAME: case-skill\nDESCRIPTION: Case handling\nTAGS: [alpha, , 'beta']\nALLOWED-TOOLS: [read, , write]\n---\n\nbody",
        );

        let result = parse_skill_md(&skill_md).expect("parse");
        assert_eq!(result.definition.name.as_str(), "case-skill");
        assert_eq!(result.definition.tags, vec!["alpha", "beta"]);
        assert_eq!(result.definition.allowed_tools, vec!["read", "write"]);
    }
}
