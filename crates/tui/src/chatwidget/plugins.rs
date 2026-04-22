use std::path::PathBuf;

use super::ChatWidget;

#[derive(Debug, Clone, Default)]
pub(super) enum PluginsCacheState {
    #[default]
    Uninitialized,
    Loading,
    Ready(()),
    Failed(String),
}

impl ChatWidget {
    pub(crate) fn add_plugins_output(&mut self) {
        self.add_info_message("Plugins are not wired yet.".to_string(), None);
        self.request_redraw();
    }

    pub(crate) fn on_plugins_loaded(&mut self, _cwd: PathBuf, result: Result<(), String>) {
        self.plugins_cache = match result {
            Ok(()) => PluginsCacheState::Ready(()),
            Err(err) => PluginsCacheState::Failed(err),
        };
        self.request_redraw();
    }

    pub(crate) fn open_plugins_loading_popup(&mut self) {
        self.add_info_message("Loading plugins...".to_string(), None);
    }

    pub(crate) fn refresh_plugins_popup_if_open(&mut self, _response: &()) {}
}
