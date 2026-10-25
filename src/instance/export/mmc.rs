// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

pub(super) fn prepare(options: &ExportOptions) -> ExportResult<ArchiveContents> {
    let mut components =
        vec![serde_json::json!({"uid": "net.minecraft", "version": options.game_version})];
    let uid = match options.loader {
        ModLoader::Vanilla => None,
        ModLoader::Forge => Some("net.minecraftforge"),
        ModLoader::Fabric => Some("net.fabricmc.fabric-loader"),
        ModLoader::NeoForge => Some("net.neoforged"),
        ModLoader::Quilt => Some("org.quiltmc.quilt-loader"),
    };
    if let Some(uid) = uid {
        components.push(serde_json::json!({"uid": uid, "version": options.loader_version}));
    }
    let pack = serde_json::json!({"formatVersion": 1, "components": components});
    let name = if options.name.starts_with('@') {
        format!("@{}", options.name)
    } else {
        options.name.clone()
    };
    let cfg = format!(
        "[General]\nConfigVersion=1.2\nInstanceType=OneSix\nname={}\n",
        serde_json::to_string(&name)?
    );
    Ok(ArchiveContents {
        manifests: vec![
            ("mmc-pack.json", serde_json::to_vec_pretty(&pack)?),
            ("instance.cfg", cfg.into_bytes()),
        ],
        referenced: BTreeSet::new(),
        prefix: ".minecraft",
    })
}
