use crate::{BundledSkillsConfig, FileSystemSkillCatalog, SkillCatalog, SkillConfig, SkillsConfig};
use pretty_assertions::assert_eq;

#[test]
fn python_manifest_honors_catalog_enablement_without_deleting_shared_assets() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let mut catalog = FileSystemSkillCatalog::with_devo_home(
        SkillsConfig::default(),
        home.path().into(),
        workspace.path().into(),
        vec![],
    );
    let initial = catalog.python_skills(Some(workspace.path())).unwrap();
    assert!(initial.iter().any(|skill| skill.module == "goal"));
    let goal = initial.iter().find(|skill| skill.module == "goal").unwrap();
    let mut config = SkillsConfig::default();
    config.config.push(SkillConfig {
        path: Some(goal.skill_path.clone()),
        name: None,
        enabled: false,
    });
    catalog.set_config(config.clone(), vec![]);
    let disabled = catalog.python_skills(Some(workspace.path())).unwrap();
    assert_eq!(
        disabled,
        initial
            .iter()
            .filter(|skill| skill.module != "goal")
            .cloned()
            .collect::<Vec<_>>()
    );
    config.bundled = Some(BundledSkillsConfig { enabled: false });
    catalog.set_config(config, vec![]);
    assert_eq!(
        catalog.python_skills(Some(workspace.path())).unwrap(),
        vec![]
    );
    assert!(goal.package_dir.join("__init__.py").is_file());
    catalog.set_config(SkillsConfig::default(), vec![]);
    assert_eq!(
        catalog.python_skills(Some(workspace.path())).unwrap(),
        initial
    );
    catalog.set_config(
        SkillsConfig {
            enabled: false,
            ..Default::default()
        },
        vec![],
    );
    assert_eq!(
        catalog.python_skills(Some(workspace.path())).unwrap(),
        vec![]
    );
}

#[test]
fn discovered_workspace_code_is_not_automatically_executed() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let package = workspace.path().join("skills/custom/src/custom");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        workspace.path().join("skills/custom/SKILL.md"),
        "---\nname: custom\ndescription: Test skill\n---\nUse explicitly.",
    )
    .unwrap();
    std::fs::write(
        package.join("__init__.py"),
        "raise RuntimeError('do not auto execute')",
    )
    .unwrap();
    let mut catalog = FileSystemSkillCatalog::with_devo_home(
        SkillsConfig {
            bundled: Some(BundledSkillsConfig { enabled: false }),
            ..Default::default()
        },
        home.path().into(),
        workspace.path().into(),
        vec![],
    );
    assert!(
        catalog
            .discover(Some(workspace.path()), true)
            .unwrap()
            .iter()
            .any(|skill| skill.name == "custom")
    );
    assert_eq!(
        catalog.python_skills(Some(workspace.path())).unwrap(),
        vec![]
    );
}
