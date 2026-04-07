/// Manages the tab bar for switching between the notification inbox
/// and project sessions. Tab 0 is always "inbox", tabs 1+ are sessions.
pub struct TabManager {
    tabs: Vec<TabEntry>,
    selected: usize,
}

pub struct TabEntry {
    pub title: String,
    /// None for the inbox tab, Some(session_id) for project tabs
    pub session_id: Option<String>,
}

impl TabManager {
    pub fn new() -> Self {
        Self {
            tabs: vec![TabEntry {
                title: "inbox".to_string(),
                session_id: None,
            }],
            selected: 0,
        }
    }

    pub fn titles(&self) -> Vec<String> {
        self.tabs.iter().map(|t| t.title.clone()).collect()
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Returns the session_id of the currently selected tab, if it's a session tab.
    pub fn active_session_id(&self) -> Option<&str> {
        self.tabs
            .get(self.selected)
            .and_then(|t| t.session_id.as_deref())
    }

    pub fn add_session(&mut self, name: String, session_id: String) {
        // Guard against duplicate tabs for the same session
        if self
            .tabs
            .iter()
            .any(|t| t.session_id.as_deref() == Some(&session_id))
        {
            return;
        }
        self.tabs.push(TabEntry {
            title: name,
            session_id: Some(session_id),
        });
        // Auto-select the first session added
        if self.tabs.len() == 2 {
            self.selected = 1;
        }
    }

    pub fn select_next(&mut self) -> Option<&str> {
        if !self.tabs.is_empty() {
            self.selected = (self.selected + 1) % self.tabs.len();
        }
        self.active_session_id()
    }

    pub fn previous(&mut self) -> Option<&str> {
        if !self.tabs.is_empty() {
            self.selected = if self.selected == 0 {
                self.tabs.len() - 1
            } else {
                self.selected - 1
            };
        }
        self.active_session_id()
    }
}

impl Default for TabManager {
    fn default() -> Self {
        Self::new()
    }
}
