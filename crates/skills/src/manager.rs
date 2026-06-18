//! Skill manager that owns root assembly, system-skill installation, and caching.

use std::collections::HashMap;
use std::collections::VecDeque;
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

const MAX_CACHED_CWDS: usize = 64;

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
    cache: RwLock<SkillLoadCache>,
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
            cache: RwLock::new(SkillLoadCache::default()),
        }
    }

    pub fn config(&self) -> SkillsRuntimeConfig {
        self.config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn include_instructions(&self) -> bool {
        let config = self
            .config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        config.enabled && config.include_instructions
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
        self.cache
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub fn skills_for_cwd(&self, cwd: &Path, force_reload: bool) -> SkillLoadOutcome {
        let cwd = canonicalize_for_identity(cwd);
        if !force_reload
            && let Some(outcome) = self
                .cache
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&cwd)
        {
            return outcome;
        }

        let config = self.config();
        if !config.enabled {
            return SkillLoadOutcome::default();
        }

        let roots = self.skill_roots(&cwd, &config);
        let outcome = load_skills_from_roots(roots, &config.config_rules);
        self.cache
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(cwd, outcome.clone());
        outcome
    }

    fn skill_roots(&self, cwd: &Path, config: &SkillsRuntimeConfig) -> Vec<SkillRoot> {
        let mut roots =
            Vec::with_capacity(config.workspace_roots.len() + config.user_roots.len() + 2);
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
        {
            let plugin_roots = self
                .plugin_roots
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            roots.reserve(plugin_roots.len());
            roots.extend(plugin_roots.iter().cloned().map(|root| SkillRoot {
                path: root.path,
                scope: SkillScope::Plugin,
                plugin_id: Some(root.plugin_id),
            }));
        }
        {
            let extra_roots = self
                .extra_roots
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            roots.reserve(extra_roots.len());
            roots.extend(extra_roots.iter().cloned().map(|path| SkillRoot {
                path,
                scope: SkillScope::User,
                plugin_id: None,
            }));
        }
        roots.extend(repo_agents_skill_roots(cwd, &config.project_root_markers));
        dedupe_roots(&mut roots);
        roots
    }
}

#[derive(Debug, Default)]
struct SkillLoadCache {
    entries: HashMap<PathBuf, SkillLoadOutcome>,
    insertion_order: VecDeque<PathBuf>,
}

impl SkillLoadCache {
    fn get(&self, cwd: &Path) -> Option<SkillLoadOutcome> {
        self.entries.get(cwd).cloned()
    }

    fn insert(&mut self, cwd: PathBuf, outcome: SkillLoadOutcome) {
        if !self.entries.contains_key(&cwd) {
            self.insertion_order.push_back(cwd.clone());
        }
        self.entries.insert(cwd, outcome);
        self.prune_oldest();
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.insertion_order.clear();
    }

    fn prune_oldest(&mut self) {
        while self.entries.len() > MAX_CACHED_CWDS {
            let Some(cwd) = self.insertion_order.pop_front() else {
                break;
            };
            self.entries.remove(&cwd);
        }
    }
}

fn workspace_native_roots<'a>(
    cwd: &'a Path,
    config: &'a SkillsRuntimeConfig,
) -> impl Iterator<Item = SkillRoot> + 'a {
    config.workspace_roots.iter().map(|root| {
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
}

fn user_native_roots<'a>(
    devo_home: &'a Path,
    config: &'a SkillsRuntimeConfig,
) -> impl Iterator<Item = SkillRoot> + 'a {
    config.user_roots.iter().map(|root| {
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
}

fn repo_agents_skill_roots(cwd: &Path, project_root_markers: &[String]) -> Vec<SkillRoot> {
    let project_root = find_project_root(cwd, project_root_markers);
    let mut roots = Vec::new();
    for dir in cwd.ancestors() {
        if !dir.starts_with(&project_root) {
            break;
        }
        let path = dir.join(".agents").join("skills");
        if path.is_dir() {
            roots.push(SkillRoot {
                path,
                scope: SkillScope::Repo,
                plugin_id: None,
            });
        }
    }
    roots.reverse();
    roots
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
    let mut seen = std::collections::HashSet::with_capacity(roots.len());
    roots.retain(|root| seen.insert(canonicalize_for_identity(&root.path)));
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn skill_load_cache_prunes_oldest_cwd() {
        let mut cache = SkillLoadCache::default();

        for index in 0..=MAX_CACHED_CWDS {
            cache.insert(
                PathBuf::from(format!("/workspace/{index}")),
                SkillLoadOutcome::default(),
            );
        }

        let state = (
            cache.entries.len(),
            cache.get(Path::new("/workspace/0")).is_some(),
            cache.get(Path::new("/workspace/1")).is_some(),
            cache
                .get(Path::new(&format!("/workspace/{MAX_CACHED_CWDS}")))
                .is_some(),
        );

        assert_eq!(state, (MAX_CACHED_CWDS, false, true, true));
    }
}
