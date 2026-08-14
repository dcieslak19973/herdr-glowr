//! The shared herdr plugin configuration boundary.
//!
//! See `docs/superpowers/specs/2026-08-13-herdr-glowr-design.md` ("Configuration"). glowr
//! takes no CLI flags of its own — running the binary with no recognized subcommand launches
//! the TUI directly, pointed at the cwd's git worktree — so this module's only job is parsing
//! and validating `$HERDR_PLUGIN_CONFIG_DIR/config.toml`.

use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

const PLUGIN_CONFIG_KEYS: [&str; 6] =
    ["theme", "toggle_placement", "toggle_direction", "auto_open", "show_ignored", "comment_sync"];

/// Where the toggle action opens the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TogglePlacement {
    Split,
    Overlay,
    Zoomed,
    Tab,
}

impl TogglePlacement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Split => "split",
            Self::Overlay => "overlay",
            Self::Zoomed => "zoomed",
            Self::Tab => "tab",
        }
    }
}

/// Direction for split placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleDirection {
    Right,
    Down,
}

impl ToggleDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Down => "down",
        }
    }
}

/// When a user (reviewer) comment persists to the shared on-disk store, making it visible to
/// the agent. Agent-authored comments are always store-resident regardless of this setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentSync {
    Immediate,
    OnSend,
}

impl CommentSync {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::OnSend => "on-send",
        }
    }
}

/// One validated snapshot of `$HERDR_PLUGIN_CONFIG_DIR/config.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginConfig {
    theme: String,
    toggle_placement: TogglePlacement,
    toggle_direction: ToggleDirection,
    auto_open: bool,
    show_ignored: bool,
    comment_sync: CommentSync,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            theme: crate::theme::DEFAULT.to_owned(),
            toggle_placement: TogglePlacement::Split,
            toggle_direction: ToggleDirection::Right,
            // A fresh worktree has no plan yet; the user opens glowr when a doc lands.
            auto_open: false,
            show_ignored: false,
            comment_sync: CommentSync::Immediate,
        }
    }
}

impl PluginConfig {
    pub fn theme(&self) -> &str {
        &self.theme
    }

    pub fn toggle_placement(&self) -> TogglePlacement {
        self.toggle_placement
    }

    pub fn toggle_direction(&self) -> ToggleDirection {
        self.toggle_direction
    }

    pub fn auto_open(&self) -> bool {
        self.auto_open
    }

    pub fn show_ignored(&self) -> bool {
        self.show_ignored
    }

    pub fn comment_sync(&self) -> CommentSync {
        self.comment_sync
    }

    /// Stable machine-readable output consumed by the shell entry points.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "theme": self.theme,
            "toggle_placement": self.toggle_placement.as_str(),
            "toggle_direction": self.toggle_direction.as_str(),
            "auto_open": self.auto_open,
            "show_ignored": self.show_ignored,
            "comment_sync": self.comment_sync.as_str(),
        })
    }
}

/// A whole-file configuration failure. It keeps the path in the value so every entry point can
/// show the same actionable diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginConfigError {
    path: PathBuf,
    detail: String,
}

impl PluginConfigError {
    fn new(path: &Path, detail: impl Into<String>) -> Self {
        Self { path: path.to_owned(), detail: detail.into() }
    }
}

impl fmt::Display for PluginConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "config {}: {}", self.path.display(), self.detail)
    }
}

impl std::error::Error for PluginConfigError {}

/// Read one plugin config snapshot from the process environment. An unset config directory is
/// standalone mode and uses defaults; a configured directory always names `config.toml`.
pub fn plugin_config() -> Result<PluginConfig, PluginConfigError> {
    let Some(dir) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") else {
        return Ok(PluginConfig::default());
    };
    plugin_config_in(dir)
}

/// Read one plugin config snapshot from `<dir>/config.toml`.
pub fn plugin_config_in(dir: impl AsRef<Path>) -> Result<PluginConfig, PluginConfigError> {
    parse_plugin_config(&dir.as_ref().join("config.toml"))
}

fn parse_plugin_config(path: &Path) -> Result<PluginConfig, PluginConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(PluginConfig::default()),
        Err(error) => {
            return Err(PluginConfigError::new(path, format!("read failed: {error}")));
        }
    };
    let table: toml::Table = text.parse().map_err(|error: toml::de::Error| {
        PluginConfigError::new(path, format!("syntax error: {}", error.message()))
    })?;
    if let Some(key) = table.keys().find(|key| !PLUGIN_CONFIG_KEYS.contains(&key.as_str())) {
        return Err(PluginConfigError::new(
            path,
            format!("unknown key {key:?}; expected one of {}", PLUGIN_CONFIG_KEYS.join(", ")),
        ));
    }

    let mut config = PluginConfig::default();
    if let Some(value) = table.get("theme") {
        let theme = string_value(path, "theme", value, "a built-in theme name")?;
        if !crate::theme::is_known(theme) {
            return Err(PluginConfigError::new(
                path,
                format!("invalid value for `theme`: {theme:?}; expected a built-in theme name"),
            ));
        }
        theme.clone_into(&mut config.theme);
    }
    if let Some(value) = table.get("toggle_placement") {
        config.toggle_placement = match string_value(
            path,
            "toggle_placement",
            value,
            "one of split, overlay, zoomed, tab",
        )? {
            "split" => TogglePlacement::Split,
            "overlay" => TogglePlacement::Overlay,
            "zoomed" => TogglePlacement::Zoomed,
            "tab" => TogglePlacement::Tab,
            _ => {
                return Err(value_error(
                    path,
                    "toggle_placement",
                    "one of split, overlay, zoomed, tab",
                ));
            }
        };
    }
    if let Some(value) = table.get("toggle_direction") {
        config.toggle_direction =
            match string_value(path, "toggle_direction", value, "one of right, down")? {
                "right" => ToggleDirection::Right,
                "down" => ToggleDirection::Down,
                _ => return Err(value_error(path, "toggle_direction", "one of right, down")),
            };
    }
    if let Some(value) = table.get("auto_open") {
        config.auto_open =
            value.as_bool().ok_or_else(|| value_error(path, "auto_open", "a boolean"))?;
    }
    if let Some(value) = table.get("show_ignored") {
        config.show_ignored =
            value.as_bool().ok_or_else(|| value_error(path, "show_ignored", "a boolean"))?;
    }
    if let Some(value) = table.get("comment_sync") {
        config.comment_sync =
            match string_value(path, "comment_sync", value, "one of immediate, on-send")? {
                "immediate" => CommentSync::Immediate,
                "on-send" => CommentSync::OnSend,
                _ => return Err(value_error(path, "comment_sync", "one of immediate, on-send")),
            };
    }
    Ok(config)
}

fn string_value<'a>(
    path: &Path,
    key: &str,
    value: &'a toml::Value,
    expected: &str,
) -> Result<&'a str, PluginConfigError> {
    value.as_str().ok_or_else(|| value_error(path, key, expected))
}

fn value_error(path: &Path, key: &str, expected: &str) -> PluginConfigError {
    PluginConfigError::new(path, format!("invalid value for `{key}`; expected {expected}"))
}

/// Print the shared normalized configuration for `herdr-glowr sidebar <mode>`.
pub fn print_plugin_config() -> Result<(), PluginConfigError> {
    println!("{}", plugin_config()?.to_json());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CommentSync, PluginConfig, ToggleDirection, TogglePlacement};

    #[test]
    fn missing_file_uses_all_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(super::plugin_config_in(dir.path()).unwrap(), PluginConfig::default());
    }

    #[test]
    fn defaults_are_split_right_no_auto_open_no_ignored_immediate() {
        let config = PluginConfig::default();
        assert_eq!(config.toggle_placement(), TogglePlacement::Split);
        assert_eq!(config.toggle_direction(), ToggleDirection::Right);
        assert!(!config.auto_open());
        assert!(!config.show_ignored());
        assert_eq!(config.comment_sync(), CommentSync::Immediate);
    }

    #[test]
    fn omitted_keys_keep_their_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "theme = \"gruvbox\"\n").unwrap();
        let config = super::plugin_config_in(dir.path()).unwrap();
        assert_eq!(config.theme(), "gruvbox");
        assert_eq!(config.toggle_placement(), TogglePlacement::Split);
        assert_eq!(config.toggle_direction(), ToggleDirection::Right);
        assert!(!config.auto_open());
        assert!(!config.show_ignored());
        assert_eq!(config.comment_sync(), CommentSync::Immediate);
    }

    #[test]
    fn reads_complete_valid_file_as_one_value() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            concat!(
                "theme = \"tokyo-night\"\n",
                "toggle_placement = \"overlay\"\n",
                "toggle_direction = \"down\"\n",
                "auto_open = true\n",
                "show_ignored = true\n",
                "comment_sync = \"on-send\"\n",
            ),
        )
        .unwrap();
        let config = super::plugin_config_in(dir.path()).unwrap();
        assert_eq!(config.theme(), "tokyo-night");
        assert_eq!(config.toggle_placement(), TogglePlacement::Overlay);
        assert_eq!(config.toggle_direction(), ToggleDirection::Down);
        assert!(config.auto_open());
        assert!(config.show_ignored());
        assert_eq!(config.comment_sync(), CommentSync::OnSend);
    }

    #[test]
    fn comment_sync_parses_both_values_and_defaults_immediate() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            super::plugin_config_in(dir.path()).unwrap().comment_sync(),
            CommentSync::Immediate
        );

        std::fs::write(dir.path().join("config.toml"), "comment_sync = \"on-send\"\n").unwrap();
        assert_eq!(
            super::plugin_config_in(dir.path()).unwrap().comment_sync(),
            CommentSync::OnSend
        );

        std::fs::write(dir.path().join("config.toml"), "comment_sync = \"sometimes\"\n").unwrap();
        assert!(super::plugin_config_in(dir.path()).is_err());
    }

    #[test]
    fn unknown_key_and_syntax_error_fail_the_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"gruvbox\"\nbase_branches = [\"main\"]\n").unwrap();
        let error = super::plugin_config_in(dir.path()).unwrap_err().to_string();
        assert!(error.contains(path.to_str().unwrap()));
        assert!(error.contains("unknown key \"base_branches\""));

        std::fs::write(&path, "theme = [\n").unwrap();
        assert!(
            super::plugin_config_in(dir.path()).unwrap_err().to_string().contains("syntax error")
        );
    }

    #[test]
    fn every_invalid_value_fails_instead_of_falling_back() {
        let cases = [
            ("theme = \"unknown\"\n", "`theme`"),
            ("toggle_placement = \"left\"\n", "`toggle_placement`"),
            ("toggle_direction = \"left\"\n", "`toggle_direction`"),
            ("auto_open = \"yes\"\n", "`auto_open`"),
            ("show_ignored = \"yes\"\n", "`show_ignored`"),
            ("comment_sync = \"sometimes\"\n", "`comment_sync`"),
        ];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for (text, key) in cases {
            std::fs::write(&path, text).unwrap();
            let error = super::plugin_config_in(dir.path()).unwrap_err().to_string();
            assert!(error.contains(key), "{text}: {error}");
            assert!(error.contains("expected"), "{text}: {error}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn unreadable_config_path_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("config.toml")).unwrap();
        let error = super::plugin_config_in(dir.path()).unwrap_err().to_string();
        assert!(error.contains("read failed"));
        assert!(error.contains("config.toml"));
    }

    #[test]
    fn normalized_json_contains_every_key() {
        let value = PluginConfig::default().to_json();
        let object = value.as_object().unwrap();
        assert_eq!(object.len(), 6);
        assert_eq!(object["toggle_placement"], "split");
        assert_eq!(object["toggle_direction"], "right");
        assert_eq!(object["auto_open"], false);
        assert_eq!(object["show_ignored"], false);
        assert_eq!(object["comment_sync"], "immediate");
    }
}
