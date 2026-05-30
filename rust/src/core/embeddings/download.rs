//! Automatic model download from HuggingFace Hub.
//!
//! Downloads the selected ONNX embedding model and its vocabulary/tokenizer
//! files on first use. Files are cached per-model in subdirectories under
//! `~/.lean-ctx/models/<model-name>/` and only downloaded once.

use std::io::Read;
use std::path::{Path, PathBuf};

use super::model_registry::{ModelConfig, VocabSource};

const USER_AGENT: &str = concat!("lean-ctx/", env!("CARGO_PKG_VERSION"));

struct DownloadFile {
    url: String,
    local_name: &'static str,
    min_bytes: u64,
}

/// Ensure all required model files are present, downloading if necessary.
/// Returns the model directory path on success.
pub fn ensure_model(model_dir: &Path, config: &ModelConfig) -> anyhow::Result<PathBuf> {
    let files = download_files(config);

    let all_present = files.iter().all(|f| model_dir.join(f.local_name).exists());

    if all_present {
        return Ok(model_dir.to_path_buf());
    }

    tracing::info!(
        "Embedding model '{}' not found, downloading to {}",
        config.name,
        model_dir.display()
    );
    std::fs::create_dir_all(model_dir)?;

    for file in &files {
        let local_path = model_dir.join(file.local_name);
        if local_path.exists() {
            let meta = std::fs::metadata(&local_path)?;
            if meta.len() >= file.min_bytes {
                tracing::debug!("{} already present ({} bytes)", file.local_name, meta.len());
                continue;
            }
            tracing::warn!(
                "{} exists but too small ({} < {}), re-downloading",
                file.local_name,
                meta.len(),
                file.min_bytes
            );
        }

        download_file(&file.url, file.local_name, file.min_bytes, model_dir)?;
    }

    verify_model_files(model_dir, config)?;

    tracing::info!(
        "Embedding model '{}' ready at {}",
        config.name,
        model_dir.display()
    );
    Ok(model_dir.to_path_buf())
}

fn download_files(config: &ModelConfig) -> Vec<DownloadFile> {
    vec![
        DownloadFile {
            url: config.model_url(),
            local_name: "model.onnx",
            min_bytes: config.model_min_bytes,
        },
        DownloadFile {
            url: config.vocab_url(),
            local_name: config.vocab_file.filename(),
            min_bytes: config.vocab_min_bytes,
        },
    ]
}

fn download_file(
    url: &str,
    local_name: &str,
    min_bytes: u64,
    model_dir: &Path,
) -> anyhow::Result<()> {
    let local_path = model_dir.join(local_name);
    let tmp_path = model_dir.join(format!("{local_name}.tmp"));

    tracing::info!("Downloading {local_name} ...");

    let response = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| anyhow::anyhow!("Failed to download {url}: {e}"))?;

    let status = response.status();
    if status != 200 {
        anyhow::bail!("Download of {local_name} returned HTTP {status}");
    }

    let content_length = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let mut body = response.into_body().into_reader();
    let mut out = std::fs::File::create(&tmp_path)?;
    let mut buf = vec![0u8; 65536];
    let mut total: u64 = 0;
    let mut last_report: u64 = 0;

    loop {
        let n = body.read(&mut buf)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut out, &buf[..n])?;
        total += n as u64;

        if total - last_report > 1_000_000 {
            if let Some(cl) = content_length {
                let pct = (total as f64 / cl as f64 * 100.0) as u32;
                tracing::info!(
                    "  {local_name} — {:.1}MB / {:.1}MB ({pct}%)",
                    total as f64 / 1_048_576.0,
                    cl as f64 / 1_048_576.0,
                );
            } else {
                tracing::info!(
                    "  {local_name} — {:.1}MB downloaded",
                    total as f64 / 1_048_576.0
                );
            }
            last_report = total;
        }
    }
    drop(out);

    if total < min_bytes {
        let _ = std::fs::remove_file(&tmp_path);
        anyhow::bail!(
            "Downloaded {local_name} is too small ({total} bytes, expected >= {min_bytes})",
        );
    }

    std::fs::rename(&tmp_path, &local_path)?;
    tracing::info!("  {local_name} — {:.1}MB saved", total as f64 / 1_048_576.0);

    Ok(())
}

fn verify_model_files(model_dir: &Path, config: &ModelConfig) -> anyhow::Result<()> {
    let model_path = model_dir.join("model.onnx");
    if !model_path.exists() {
        anyhow::bail!("Model file model.onnx missing after download");
    }
    let meta = std::fs::metadata(&model_path)?;
    if meta.len() < config.model_min_bytes {
        anyhow::bail!(
            "Model file model.onnx is corrupt ({} bytes, expected >= {})",
            meta.len(),
            config.model_min_bytes
        );
    }

    let vocab_name = config.vocab_file.filename();
    let vocab_path = model_dir.join(vocab_name);
    if !vocab_path.exists() {
        anyhow::bail!("Vocab file {vocab_name} missing after download");
    }
    let vmeta = std::fs::metadata(&vocab_path)?;
    if vmeta.len() < config.vocab_min_bytes {
        anyhow::bail!(
            "Vocab file {vocab_name} is corrupt ({} bytes, expected >= {})",
            vmeta.len(),
            config.vocab_min_bytes
        );
    }

    if let VocabSource::VocabTxt(_) = config.vocab_file {
        let content = std::fs::read_to_string(&vocab_path)?;
        let line_count = content.lines().count();
        if line_count < 20_000 {
            anyhow::bail!(
                "{vocab_name} appears corrupt ({line_count} lines, expected ~30K for BERT)"
            );
        }
    }

    Ok(())
}

/// Remove all downloaded model files (for cleanup/re-download).
pub fn clean_model(model_dir: &Path) -> anyhow::Result<()> {
    for name in ["model.onnx", "vocab.txt", "tokenizer.json"] {
        let path = model_dir.join(name);
        if path.exists() {
            std::fs::remove_file(&path)?;
            tracing::info!("Removed {}", path.display());
        }
        let tmp_path = model_dir.join(format!("{name}.tmp"));
        if tmp_path.exists() {
            std::fs::remove_file(&tmp_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::embeddings::model_registry::EmbeddingModel;

    #[test]
    fn download_files_all_models() {
        for model in EmbeddingModel::ALL {
            let cfg = model.config();
            let files = download_files(&cfg);
            assert_eq!(
                files.len(),
                2,
                "model={} should have 2 download files",
                cfg.name
            );
            assert!(files[0].url.contains("model.onnx"));
            assert!(files[0].min_bytes > 0);
        }
    }

    #[test]
    fn model_urls_are_https() {
        for model in EmbeddingModel::ALL {
            let cfg = model.config();
            let files = download_files(&cfg);
            for f in &files {
                assert!(
                    f.url.starts_with("https://"),
                    "URL for {} must be HTTPS: {}",
                    cfg.name,
                    f.url
                );
            }
        }
    }

    #[test]
    fn verify_fails_on_empty_dir() {
        let dir = std::env::temp_dir().join("lean_ctx_test_verify_empty_v2");
        let _ = std::fs::create_dir_all(&dir);
        let cfg = EmbeddingModel::AllMiniLmL6V2.config();
        assert!(verify_model_files(&dir, &cfg).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
