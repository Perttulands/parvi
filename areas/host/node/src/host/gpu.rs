//! NVIDIA GPUs through `nvidia-smi`, where it is installed: the driver puts
//! it in `System32` on Windows and ships it outside `PATH` on WSL, so those
//! places are tried too (CHROTE's list on WSL).

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::reading::{Gpu, Usage};

#[cfg(not(windows))]
const CANDIDATES: [&str; 2] = [
    "/usr/lib/wsl/lib/nvidia-smi",
    "/mnt/c/Windows/System32/lxss/lib/nvidia-smi",
];

#[cfg(windows)]
const CANDIDATES: [&str; 2] = [
    r"C:\Windows\System32\nvidia-smi.exe",
    r"C:\Program Files\NVIDIA Corporation\NVSMI\nvidia-smi.exe",
];

const QUERY: &str = "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu";

/// How long one query may take before the GPU counts as not reported.
const TIMEOUT: Duration = Duration::from_millis(1200);

/// Where `nvidia-smi` is, if anywhere: on `PATH` first, then the driver's
/// places.
pub fn locate() -> Option<PathBuf> {
    locate_in(std::env::var_os("PATH"), &CANDIDATES)
}

fn locate_in(path: Option<std::ffi::OsString>, candidates: &[&str]) -> Option<PathBuf> {
    let on_path = path
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|dir| dir.join(format!("nvidia-smi{}", std::env::consts::EXE_SUFFIX)));
    on_path
        .chain(candidates.iter().map(PathBuf::from))
        .find(|path| path.is_file())
}

/// Every GPU `nvidia-smi` reports now. Empty if it fails or says nothing.
pub async fn query(nvidia_smi: &Path) -> Result<Vec<Gpu>, String> {
    let mut command = tokio::process::Command::new(nvidia_smi);
    // A node without a console must not flash one every reading.
    #[cfg(windows)]
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    let output = command
        .args([QUERY, "--format=csv,noheader,nounits"])
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(TIMEOUT, output).await {
        Ok(Ok(output)) if output.status.success() => {
            Ok(parse(&String::from_utf8_lossy(&output.stdout)))
        }
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let line = stderr.lines().next().unwrap_or("").trim();
            Err(format!(
                "nvidia-smi {}{}",
                output.status,
                if line.is_empty() {
                    String::new()
                } else {
                    format!(": {line}")
                }
            ))
        }
        Ok(Err(error)) => Err(format!("nvidia-smi: {error}")),
        Err(_) => Err("nvidia-smi timed out".into()),
    }
}

pub async fn read(path: Option<&Path>) -> Result<Vec<Gpu>, String> {
    match path {
        Some(path) => query(path).await,
        None => Err("nvidia-smi not found".into()),
    }
}

/// One GPU per CSV line: name, utilization %, memory used and total in MiB,
/// temperature °C. `N/A` reads as unknown.
pub fn parse(csv: &str) -> Vec<Gpu> {
    csv.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            let [name, util, used, total, temp] = fields[..] else {
                return None;
            };
            let number = |field: &str| field.parse::<f32>().ok();
            let mib = |field: &str| field.parse::<u64>().ok().map(|mib| mib * 1024 * 1024);
            Some(Gpu {
                name: name.to_owned(),
                util: number(util),
                memory: mib(used)
                    .zip(mib(total))
                    .map(|(used, total)| Usage { used, total }),
                temp: number(temp),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn missing_and_failed_commands_explain_why() {
        use std::os::unix::fs::PermissionsExt;
        let dir = parvi_testkit::fresh_dir("gpu-failure");
        let missing = locate_in(Some(dir.clone().into_os_string()), &[]);
        assert_eq!(
            read(missing.as_deref()).await.unwrap_err(),
            "nvidia-smi not found"
        );
        let path = dir.join("nvidia-smi");
        std::fs::write(
            &path,
            "#!/bin/sh\necho 'NVIDIA-SMI has failed' >&2\necho ignored >&2\nexit 9\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let reason = query(&path).await.unwrap_err();
        assert!(reason.contains("9") && reason.contains("NVIDIA-SMI has failed"));
        assert!(!reason.contains("ignored"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn each_line_is_one_gpu_and_na_is_unknown() {
        let gpus = parse(
            "NVIDIA GeForce RTX 4090, 87, 18636, 24564, 61\nNVIDIA T4, [N/A], N/A, 15360, N/A\n",
        );
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(gpus[0].util, Some(87.0));
        assert_eq!(
            gpus[0].memory,
            Some(Usage {
                used: 18636 << 20,
                total: 24564 << 20
            })
        );
        assert_eq!(gpus[0].temp, Some(61.0));
        assert_eq!(
            (gpus[1].util, gpus[1].memory, gpus[1].temp),
            (None, None, None)
        );
    }

    #[test]
    fn a_line_of_the_wrong_shape_is_skipped() {
        assert!(parse("No devices were found\n").is_empty());
        assert!(parse("").is_empty());
    }
}
