//! Viewport delegation from the shell state to the active module.

use super::Workbench;
use crate::viewport::{ViewportInput, ViewportModel};

impl Workbench {
    /// Whether the active module/nav renders a bounded spatial viewport in
    /// place of the generic reading pane.
    pub fn has_active_viewport(&self) -> bool {
        self.registry
            .get(self.active)
            .is_some_and(|m| m.has_viewport(self.active_nav()))
    }

    /// The already-culled viewport projection for the active module/nav.
    pub fn active_viewport(&self) -> Option<ViewportModel> {
        self.registry
            .get(self.active)
            .and_then(|m| m.viewport(self.active_nav()))
    }

    /// Route canvas input to the active module. The nav key is cloned before
    /// the mutable module lookup; nav strings are small and stable per frame.
    pub fn viewport_input(&mut self, input: ViewportInput) -> bool {
        let nav = self.active_nav().map(str::to_string);
        self.registry
            .get_mut(self.active)
            .is_some_and(|m| m.viewport_input(nav.as_deref(), input))
    }
}
