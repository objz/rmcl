// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackFormat {
    CurseForge,
    Mrpack,
    Mmc,
}

impl PackFormat {
    pub const ALL: [Self; 3] = [Self::CurseForge, Self::Mrpack, Self::Mmc];

    pub fn label(self) -> &'static str {
        match self {
            Self::CurseForge => "CurseForge",
            Self::Mrpack => "Modrinth",
            Self::Mmc => "MultiMC/Prism",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Mrpack => "mrpack",
            Self::CurseForge | Self::Mmc => "zip",
        }
    }
}
