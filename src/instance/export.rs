use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::ModLoader;
use super::content::manifest::fingerprint;
use super::import::curseforge::{Manifest, ManifestFile, ManifestLoader, Minecraft};

type ExportResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub struct ExportOptions {
    pub name: String,
    pub version: String,
    pub author: String,
    pub game_version: String,
    pub loader: ModLoader,
    pub loader_version: Option<String>,
    pub include: Vec<PathBuf>,
    pub bundle_unmatched: bool,
}

const DEFAULT_PATHS: &[&str] = &[
    "mods",
    "config",
    "defaultconfigs",
    "kubejs",
    "scripts",
    "resourcepacks",
    "shaderpacks",
    "datapacks",
];

fn collect(root: &Path, relative: &Path, files: &mut BTreeSet<PathBuf>) -> ExportResult<()> {
    if !relative.to_str().is_some_and(|path| {
        super::import::portable_pack_path(path)
            .is_ok_and(|normalized| cfg!(windows) || normalized == path)
    }) {
        return Err(format!("Invalid export path: {}", relative.display()).into());
    }
    let mut ancestor = root.to_path_buf();
    for component in relative.components() {
        ancestor.push(component);
        if std::fs::symlink_metadata(&ancestor)?
            .file_type()
            .is_symlink()
        {
            return Err(format!("Export does not follow symlinks: {}", relative.display()).into());
        }
    }
    let path = root.join(relative);
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            collect(root, &relative.join(entry?.file_name()), files)?;
        }
    } else if metadata.is_file() {
        if !relative.to_string_lossy().ends_with(".disabled") {
            files.insert(relative.to_owned());
        }
    } else {
        return Err(format!("Not a regular file: {}", path.display()).into());
    }
    Ok(())
}

fn selected_files(root: &Path, include: &[PathBuf]) -> ExportResult<BTreeSet<PathBuf>> {
    let mut files = BTreeSet::new();
    for name in DEFAULT_PATHS {
        match std::fs::symlink_metadata(root.join(name)) {
            Ok(_) => collect(root, Path::new(name), &mut files)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    for path in include {
        collect(root, path, &mut files)?;
    }
    Ok(files)
}

pub async fn export_curseforge(
    root: &Path,
    output: &Path,
    options: &ExportOptions,
) -> ExportResult<()> {
    for (label, value) in [
        ("Pack name", &options.name),
        ("Pack version", &options.version),
        ("Minecraft version", &options.game_version),
    ] {
        if value.trim().is_empty() {
            return Err(format!("{label} must not be empty").into());
        }
    }
    if options.loader != ModLoader::Vanilla
        && options
            .loader_version
            .as_ref()
            .is_none_or(|version| version.trim().is_empty())
    {
        return Err("A loader version is required for a modded pack".into());
    }
    let root = root.canonicalize()?;
    if !root.is_dir() {
        return Err("Game folder must be a directory".into());
    }
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if parent.canonicalize()?.starts_with(&root) {
        return Err("Export ZIP must be outside the game folder".into());
    }
    if output.exists() {
        return Err(format!("Output already exists: {}", output.display()).into());
    }
    let files = selected_files(&root, &options.include)?;
    let mut hashes = BTreeMap::new();
    let mut sha1_hashes = BTreeMap::new();
    for path in &files {
        if path.parent() == Some(Path::new("mods"))
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
        {
            let hash = fingerprint(&root.join(path))?;
            let value = hash
                .hash("curseforge")
                .ok_or("Missing CurseForge fingerprint")?
                .parse::<u32>()?;
            sha1_hashes.insert(
                path.clone(),
                hash.hash("sha1")
                    .ok_or("Missing SHA-1 fingerprint")?
                    .to_owned(),
            );
            hashes.insert(path.clone(), value);
        }
    }
    let mut matches = BTreeMap::new();
    if !hashes.is_empty() {
        let key =
            crate::net::curseforge::api_key().ok_or("CurseForge API key is not configured")?;
        let client = crate::net::HttpClient::new();
        let values = hashes
            .values()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        for chunk in values.chunks(100) {
            for (hash, version) in
                crate::net::curseforge::resolve_fingerprint_versions(&client, key, chunk).await?
            {
                matches.insert(hash, version);
            }
        }
    }
    let mut resolved = BTreeMap::new();
    for (path, hash) in &hashes {
        if let Some(version) = matches.get(hash)
            && version.files.iter().any(|artifact| {
                artifact
                    .hashes
                    .get("sha1")
                    .is_some_and(|expected| expected.eq_ignore_ascii_case(&sha1_hashes[path]))
            })
        {
            resolved.insert(
                path.clone(),
                (version.project_id.parse()?, version.id.parse()?),
            );
        }
    }
    write_archive(&root, output, options, &files, &resolved)
}

fn write_archive(
    root: &Path,
    output: &Path,
    options: &ExportOptions,
    files: &BTreeSet<PathBuf>,
    resolved: &BTreeMap<PathBuf, (u64, u64)>,
) -> ExportResult<()> {
    let unmatched = files
        .iter()
        .filter(|path| path.starts_with("mods") && !resolved.contains_key(*path))
        .collect::<Vec<_>>();
    if !unmatched.is_empty() {
        let names = unmatched
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        if !options.bundle_unmatched {
            return Err(format!("Mods without CurseForge matches:\n{names}\nUse --bundle-unmatched to include these files in overrides/mods.").into());
        }
        eprintln!("Bundling custom/unmatched mods:\n{names}");
    }
    let loaders = match options.loader {
        ModLoader::Vanilla => Vec::new(),
        loader => vec![ManifestLoader {
            id: format!(
                "{}-{}",
                loader.to_string().to_lowercase(),
                options
                    .loader_version
                    .as_deref()
                    .ok_or("Missing loader version")?
            ),
            primary: true,
        }],
    };
    let references = resolved
        .values()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|(project_id, file_id)| ManifestFile {
            project_id,
            file_id,
            required: true,
        })
        .collect::<Vec<_>>();
    let manifest = Manifest {
        minecraft: Minecraft {
            version: options.game_version.clone(),
            mod_loaders: loaders,
        },
        manifest_type: "minecraftModpack".to_owned(),
        manifest_version: 1,
        name: options.name.clone(),
        version: options.version.clone(),
        author: options.author.clone(),
        files: references,
        overrides: "overrides".to_owned(),
    };
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut zip = zip::ZipWriter::new(temporary.as_file_mut());
        let zip_options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("manifest.json", zip_options)?;
        zip.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
        for path in files {
            if resolved.contains_key(path) {
                continue;
            }
            let name = path
                .to_str()
                .ok_or("Export paths must be UTF-8")?
                .replace('\\', "/");
            zip.start_file(format!("overrides/{name}"), zip_options)?;
            std::io::copy(&mut std::fs::File::open(root.join(path))?, &mut zip)?;
        }
        zip.finish()?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(output)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/export.rs"]
mod tests;
