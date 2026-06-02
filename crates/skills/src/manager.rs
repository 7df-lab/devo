//! Skill manager that owns root assembly, system-skill installation, and caching.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::RwLock;

use crate::config_rules::SkillConfigRules;
use crate::loader::SkillRoot;
use crate::loader::load_skills_from_roots;
use crate::model::SkillLoadOutcome;
use crate::model::SkillScope;
use crate::model::canonicalize_for_identity;
use crate::system::install_system_skills;
use crate::system::system_cache_root_dir;
use crate::system::uninstall_system_skills;

/// Minimal skill config shape consumed by `devo-skills`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillsRuntimeConfig {
    pub enabled: bool,
    pub user_roots: Vec<PathBuf>,
    pub workspace_roots: Vec<PathBuf>,
    pub include_instructions: bool,
    pub bundled_enabled: bool,
    pub config_rules: SkillConfigRules,
    pub project_root_markers: Vec<String>,
}

impl Default for SkillsRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            user_roots: vec![PathBuf::from("skills")],
            workspace_roots: vec![PathBuf::from("skills")],
            include_instructions: true,
            bundled_enabled: true,
            config_rules: SkillConfigRules::default(),
            project_root_markers: vec![".git".to_string()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSkillRoot {
    pub path: PathBuf,
    pub plugin_id: String,
}

#[derive(Debug)]
pub struct SkillsManager {
    devo_home: PathBuf,
    config: RwLock<SkillsRuntimeConfig>,
    plugin_roots: RwLock<Vec<PluginSkillRoot>>,
    extra_roots: RwLock<Vec<PathBuf>>,
    cache_by_cwd: RwLock<HashMap<PathBuf, SkillLoadOutcome>>,
}

impl SkillsManager {
    pub fn new(devo_home: PathBuf, config: SkillsRuntimeConfig) -> Self {
        if config.bundled_enabled {
            if let Err(error) = install_system_skills(&devo_home) {
                tracing::warn!(error = %error, "failed to install system skills");
            }
        } else {
            uninstall_system_skills(&devo_home);
        }
        Self {
            devo_home,
            config: RwLock::new(config),
            plugin_roots: RwLock::new(Vec::new()),
            extra_roots: RwLock::new(Vec::new()),
            cache_by_cwd: RwLock::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> SkillsRuntimeConfig {
        self.config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn include_instructions(&self) -> bool {
        self.config().enabled && self.config().include_instructions
    }

    pub fn set_config(&self, config: SkillsRuntimeConfig) {
        if config.bundled_enabled {
            if let Err(error) = install_system_skills(&self.devo_home) {
                tracing::warn!(error = %error, "failed to install system skills");
            }
        } else {
            uninstall_system_skills(&self.devo_home);
        }
        {
            let mut guard = self
                .config
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard = config;
        }
        self.clear_cache();
    }

    pub fn set_plugin_roots(&self, roots: Vec<PluginSkillRoot>) {
        {
            let mut guard = self
                .plugin_roots
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard = roots;
        }
        self.clear_cache();
    }

    pub fn set_extra_roots(&self, roots: Vec<PathBuf>) {
        {
            let mut guard = self
                .extra_roots
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard = roots;
        }
        self.clear_cache();
    }

    pub fn clear_cache(&self) {
        self.cache_by_cwd
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub fn skills_for_cwd(&self, cwd: &Path, force_reload: bool) -> SkillLoadOutcome {
        let cwd = canonicalize_for_identity(cwd);
        if !force_reload
            && let Some(outcome) = self
                .cache_by_cwd
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&cwd)
                .cloned()
        {
            return outcome;
        }

        let config = self.config();
        if !config.enabled {
            return SkillLoadOutcome::default();
        }

        let roots = self.skill_roots(&cwd, &config);
        let outcome = load_skills_from_roots(roots, &config.config_rules);
        self.cache_by_cwd
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(cwd, outcome.clone());
        outcome
    }

    fn skill_roots(&self, cwd: &Path, config: &SkillsRuntimeConfig) -> Vec<SkillRoot> {
        let mut roots = Vec::new();
        roots.extend(workspace_native_roots(cwd, config));
        roots.extend(user_native_roots(&self.devo_home, config));
        if let Some(home) = home_dir() {
            roots.push(SkillRoot {
                path: home.join(".agents").join("skills"),
                scope: SkillScope::User,
                plugin_id: None,
            });
        }
        if config.bundled_enabled {
            roots.push(SkillRoot {
                path: system_cache_root_dir(&self.devo_home),
                scope: SkillScope::System,
                plugin_id: None,
            });
        }
        roots.extend(
            self.plugin_roots
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .cloned()
                .map(|root| SkillRoot {
                    path: root.path,
                    scope: SkillScope::Plugin,
                    plugin_id: Some(root.plugin_id),
                }),
        );
        roots.extend(
            self.extra_roots
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .cloned()
                .map(|path| SkillRoot {
                    path,
                    scope: SkillScope::User,
                    plugin_id: None,
                }),
        );
        roots.extend(repo_agents_skill_roots(cwd, &config.project_root_markers));
        dedupe_roots(&mut roots);
        roots
    }
}

fn workspace_native_roots(cwd: &Path, config: &SkillsRuntimeConfig) -> Vec<SkillRoot> {
    config
        .workspace_roots
        .iter()
        .map(|root| {
            let path = if root.is_absolute() {
                root.clone()
            } else {
                cwd.join(".devo").join(root)
            };
            SkillRoot {
                path,
                scope: SkillScope::Repo,
                plugin_id: None,
            }
        })
        .collect()
}

fn user_native_roots(devo_home: &Path, config: &SkillsRuntimeConfig) -> Vec<SkillRoot> {
    config
        .user_roots
        .iter()
        .map(|root| {
            let path = if root.is_absolute() {
                root.clone()
            } else {
                devo_home.join(root)
            };
            SkillRoot {
                path,
                scope: SkillScope::User,
                plugin_id: None,
            }
        })
        .collect()
}

fn repo_agents_skill_roots(cwd: &Path, project_root_markers: &[String]) -> Vec<SkillRoot> {
    let project_root = find_project_root(cwd, project_root_markers);
    let mut ancestors = cwd.ancestors().collect::<Vec<_>>();
    ancestors.reverse();
    ancestors
        .into_iter()
        .filter(|dir| dir.starts_with(&project_root))
        .map(|dir| dir.join(".agents").join("skills"))
        .filter(|path| path.is_dir())
        .map(|path| SkillRoot {
            path,
            scope: SkillScope::Repo,
            plugin_id: None,
        })
        .collect()
}

fn find_project_root(cwd: &Path, project_root_markers: &[String]) -> PathBuf {
    if project_root_markers.is_empty() {
        return cwd.to_path_buf();
    }
    for ancestor in cwd.ancestors() {
        for marker in project_root_markers {
            if ancestor.join(marker).exists() {
                return ancestor.to_path_buf();
            }
        }
    }
    cwd.to_path_buf()
}

fn dedupe_roots(roots: &mut Vec<SkillRoot>) {
    let mut seen = std::collections::HashSet::new();
    roots.retain(|root| seen.insert(canonicalize_for_identity(&root.path)));
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
