use anyhow::{Context, Result};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const HF_REPO: &str = "istupakov/parakeet-tdt-0.6b-v3-onnx";
const FILES: &[&str] = &[
    "config.json",
    "encoder-model.onnx",
    "encoder-model.onnx.data",
    "decoder_joint-model.onnx",
    "nemo128.onnx",
    "vocab.txt",
];

pub fn ensure_model(dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir).with_context(|| format!("create_dir_all {}", dir.display()))?;

    for name in FILES {
        let dst = dir.join(name);
        if dst.exists() {
            continue;
        }
        let url = format!("https://huggingface.co/{}/resolve/main/{}", HF_REPO, name);
        eprintln!("[sidecar] downloading {name} from {url}");
        download_to(&url, &dst)?;
    }
    Ok(dir.to_path_buf())
}

fn download_to(url: &str, dst: &Path) -> Result<()> {
    let tmp = dst.with_extension("download");
    let client = reqwest::blocking::Client::builder()
        .user_agent("parakeet-sidecar/0.1")
        .timeout(std::time::Duration::from_secs(60 * 60))
        .build()?;
    let mut resp = client.get(url).send()?.error_for_status()?;
    let total = resp.content_length().unwrap_or(0);
    let mut file = fs::File::create(&tmp)?;
    let mut buf = vec![0u8; 1 << 20]; // 1 MiB
    let mut written: u64 = 0;
    let mut last_pct = -1i64;
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        written += n as u64;
        if total > 0 {
            let pct = (written * 100 / total) as i64;
            if pct != last_pct && pct % 10 == 0 {
                eprintln!(
                    "[sidecar]   {:>3}% ({written}/{total})",
                    pct,
                );
                last_pct = pct;
            }
        }
    }
    drop(file);
    fs::rename(&tmp, dst).with_context(|| format!("rename {} -> {}", tmp.display(), dst.display()))?;
    Ok(())
}
