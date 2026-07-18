use super::ProviderConfigSection;

impl ProviderConfigSection {
    /// Returns whether both sections have identical provider runtime settings.
    ///
    /// This compares every field that historically participated in full section
    /// equality except `model_overrides`. Those overrides shape catalog metadata
    /// only, so changing them does not require rebuilding a provider or router.
    pub fn is_operationally_equivalent_to(&self, other: &Self) -> bool {
        let Self {
            defaults: left_defaults,
            model_provider: left_model_provider,
            model: left_model,
            model_reasoning_effort_selection: left_reasoning_effort,
            model_auto_compact_token_limit: left_auto_compact_token_limit,
            model_context_window: left_context_window,
            disable_response_storage: left_disable_response_storage,
            preferred_auth_method: left_preferred_auth_method,
            providers: left_providers,
            model_bindings: left_model_bindings,
            model_overrides: _,
            model_providers: left_model_providers,
        } = self;
        let Self {
            defaults: right_defaults,
            model_provider: right_model_provider,
            model: right_model,
            model_reasoning_effort_selection: right_reasoning_effort,
            model_auto_compact_token_limit: right_auto_compact_token_limit,
            model_context_window: right_context_window,
            disable_response_storage: right_disable_response_storage,
            preferred_auth_method: right_preferred_auth_method,
            providers: right_providers,
            model_bindings: right_model_bindings,
            model_overrides: _,
            model_providers: right_model_providers,
        } = other;

        left_defaults == right_defaults
            && left_model_provider == right_model_provider
            && left_model == right_model
            && left_reasoning_effort == right_reasoning_effort
            && left_auto_compact_token_limit == right_auto_compact_token_limit
            && left_context_window == right_context_window
            && left_disable_response_storage == right_disable_response_storage
            && left_preferred_auth_method == right_preferred_auth_method
            && left_providers == right_providers
            && left_model_bindings == right_model_bindings
            && left_model_providers == right_model_providers
    }
}
