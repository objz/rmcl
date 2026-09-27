// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

mod render;
mod state;

#[cfg(test)]
pub use render::render;
pub use render::{popup_rect, render_with_picker};
pub use state::{
    ImportResult, ImportStep, ImportWizardState, discovery_activity, drain, handle_discovery_click,
    handle_key, has_version_popup, open, take_result,
};

#[cfg(test)]
#[path = "../../../tests/widgets/popups/import_modpack/support.rs"]
mod test_support;
#[cfg(test)]
pub(crate) use test_support::reset as reset_for_test;
