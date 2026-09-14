// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// shared network loading for Minecraft and mod-loader version pickers.

use crate::instance::{
    loader::{GameVersion, get_installer},
    models::ModLoader,
};

pub async fn game_versions(loader: ModLoader) -> Result<Vec<GameVersion>, String> {
    let client = crate::net::HttpClient::new();
    let mut versions = get_installer(loader)
        .get_game_versions(&client)
        .await
        .map_err(|error| error.to_string())?;
    versions.sort_by(|a, b| super::compare_game_versions(&b.id, &a.id));
    Ok(versions)
}

pub async fn loader_versions(loader: ModLoader, game_version: &str) -> Result<Vec<String>, String> {
    let client = crate::net::HttpClient::new();
    get_installer(loader)
        .get_versions(&client, game_version)
        .await
        .map_err(|error| error.to_string())
}
