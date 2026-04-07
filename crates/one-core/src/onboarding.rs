use std::path::PathBuf;

use anyhow::Result;

use crate::config::AppConfig;

/// Whether the user has completed first-run onboarding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingState {
    /// Config exists and `has_completed_onboarding` is true.
    Complete,
    /// Config is missing or onboarding has not been completed.
    Needed,
}

/// Check onboarding state by inspecting `~/.one/config.toml`.
pub fn check_onboarding() -> OnboardingState {
    let config_path = config_path();

    if !config_path.exists() {
        return OnboardingState::Needed;
    }

    match std::fs::read_to_string(&config_path) {
        Ok(contents) => match toml::from_str::<AppConfig>(&contents) {
            Ok(config) if config.has_completed_onboarding => OnboardingState::Complete,
            _ => OnboardingState::Needed,
        },
        Err(_) => OnboardingState::Needed,
    }
}

/// Mark onboarding as complete in the config file.
/// Loads the existing config (or default), sets the flag, and saves.
pub fn mark_onboarding_complete(config: &mut AppConfig) -> Result<()> {
    config.has_completed_onboarding = true;
    config.save()?;
    Ok(())
}

fn config_path() -> PathBuf {
    dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".one")
        .join("config.toml")
}
