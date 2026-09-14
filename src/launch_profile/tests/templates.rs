// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use std::path::PathBuf;

// owns the path buffers so tests don't have to declare them inline; the
// ctx() method borrows from self to build a TemplateContext with the
// standard set of values. windows() returns a fixture with backslash
// paths so the OS-independence test stays self-contained.
struct Fixture {
    lib: PathBuf,
    nat: PathBuf,
    game: PathBuf,
    assets: PathBuf,
    user_properties: String,
}

impl Fixture {
    fn unix() -> Self {
        Self {
            lib: PathBuf::from("/m/libraries"),
            nat: PathBuf::from("/m/natives"),
            game: PathBuf::from("/i/.minecraft"),
            assets: PathBuf::from("/m/assets"),
            user_properties: "{}".to_string(),
        }
    }

    fn windows() -> Self {
        Self {
            lib: PathBuf::from(r"C:\Users\test\.minecraft\libraries"),
            nat: PathBuf::from(r"C:\Users\test\.minecraft\natives"),
            game: PathBuf::from(r"C:\Users\test\.minecraft"),
            assets: PathBuf::from(r"C:\Users\test\.minecraft\assets"),
            user_properties: "{}".to_string(),
        }
    }

    fn ctx(&self) -> TemplateContext<'_> {
        TemplateContext {
            library_directory: &self.lib,
            classpath_separator: ":",
            version_name: "1.20.1",
            version_type: "release",
            natives_directory: &self.nat,
            classpath: "a.jar:b.jar",
            game_directory: &self.game,
            assets_root: &self.assets,
            assets_index_name: "5",
            auth_player_name: "Player",
            auth_uuid: "00000000-0000-0000-0000-000000000000",
            auth_access_token: "token",
            auth_xuid: "0",
            user_type: "msa",
            user_properties: &self.user_properties,
            launcher_name: "rmcl",
            launcher_version: "0.3.0",
            clientid: "0",
            quick_play_singleplayer: None,
            resolution_width: None,
            resolution_height: None,
        }
    }
}

#[rstest::rstest]
#[case::no_placeholders("--add-modules ALL-MODULE-PATH", "--add-modules ALL-MODULE-PATH")]
#[case::single_known("v=${version_name}", "v=1.20.1")]
#[case::unknown_placeholder("x=${not_a_real_var}y", "x=${not_a_real_var}y")]
#[case::unclosed_placeholder("--prefix ${unclosed", "--prefix ${unclosed")]
#[case::dollar_without_brace("$$ literal $5 $", "$$ literal $5 $")]
#[case::multiple("${version_name}-${auth_player_name}", "1.20.1-Player")]
#[case::path(
    "-DlibraryDirectory=${library_directory}",
    "-DlibraryDirectory=/m/libraries"
)]
#[case::empty_input("", "")]
fn substitute_handles(#[case] input: &str, #[case] expected: &str) {
    let fx = Fixture::unix();
    assert_eq!(substitute(input, &fx.ctx()), expected);
}

#[test]
fn substituted_value_is_not_recursively_substituted() {
    // simulate a user_properties value that happens to contain a ${...}
    // pattern. it should NOT trigger another substitution pass.
    let mut fx = Fixture::unix();
    fx.user_properties = "${version_name}".to_string();
    assert_eq!(
        substitute("${user_properties}", &fx.ctx()),
        "${version_name}"
    );
}

#[test]
fn windows_style_backslashes_in_value_pass_through() {
    // simulate a Windows install where library_directory is a path with
    // backslashes. the substitution must not interpret backslashes as
    // escape sequences or do anything else clever - it just copies the
    // value into the output.
    let fx = Fixture::windows();
    let result = substitute("-Dpath=${library_directory}", &fx.ctx());
    assert!(
        result.contains(r"C:\Users\test\.minecraft\libraries"),
        "expected backslashes preserved, got: {result}"
    );
}

#[test]
fn quick_play_world_is_substituted_when_present() {
    let fx = Fixture::unix();
    let mut context = fx.ctx();
    context.quick_play_singleplayer = Some("New World");
    assert_eq!(
        substitute("${quickPlaySingleplayer}", &context),
        "New World"
    );
}

#[test]
fn custom_resolution_is_substituted_when_present() {
    let fx = Fixture::unix();
    let mut context = fx.ctx();
    context.resolution_width = Some("1920");
    context.resolution_height = Some("1080");
    assert_eq!(
        substitute("${resolution_width}x${resolution_height}", &context),
        "1920x1080"
    );
}
