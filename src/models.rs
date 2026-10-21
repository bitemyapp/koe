use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Deserialize)]
pub struct Catalog {
    pub repository: String,
    pub revision: String,
    pub models: Vec<Model>,
}
#[derive(Deserialize)]
pub struct Model {
    pub name: String,
    pub sha256: String,
    pub bytes: u64,
}
pub fn catalog() -> Catalog {
    serde_json::from_str(include_str!("../data/models.json")).expect("bundled catalog is valid")
}
pub fn directory() -> Result<PathBuf> {
    Ok(crate::config::data_dir()?.join("models"))
}
pub fn verify(path: &Path, model: &Model) -> Result<()> {
    let mut file = std::fs::File::open(path)?;
    ensure!(
        file.metadata()?.len() == model.bytes,
        "model size does not match the catalog"
    );
    let mut hasher = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hasher.update(&bytes[..n]);
    }
    ensure!(
        format!("{:x}", hasher.finalize()) == model.sha256,
        "model SHA-256 does not match the catalog"
    );
    Ok(())
}
pub async fn download(name: &str, dir: &Path) -> Result<PathBuf> {
    let catalog = catalog();
    let model = catalog
        .models
        .iter()
        .find(|m| m.name == name)
        .context("unknown model; use koe model list")?;
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(format!("ggml-{name}.bin"));
    if dest.exists() {
        verify(&dest, model)
            .context("existing model failed verification; remove it to download again")?;
        return Ok(dest);
    }
    let url = format!(
        "https://huggingface.co/{}/resolve/{}/ggml-{name}.bin",
        catalog.repository, catalog.revision
    );
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(1800))
        .build()?;
    let mut response = client.get(url).send().await?.error_for_status()?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    let mut hash = Sha256::new();
    let mut count = 0_u64;
    eprintln!(
        "Downloading {name} ({:.1} MiB), pinned SHA-256 {}",
        model.bytes as f64 / 1048576.0,
        model.sha256
    );
    while let Some(chunk) = response.chunk().await? {
        count += chunk.len() as u64;
        ensure!(
            count <= model.bytes,
            "model download exceeded expected size"
        );
        file.write_all(&chunk)?;
        hash.update(&chunk);
    }
    ensure!(
        count == model.bytes && format!("{:x}", hash.finalize()) == model.sha256,
        "download failed size or SHA-256 verification"
    );
    file.as_file().sync_all()?;
    file.persist_noclobber(&dest)
        .context("cannot install downloaded model; another download may have completed")?;
    Ok(dest)
}
