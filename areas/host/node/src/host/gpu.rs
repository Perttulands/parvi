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
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|dir| dir.join(format!("nvidia-smi{}", std::env::consts::EXE_SUFFIX)));
    on_path
        .chain(CANDIDATES.iter().map(PathBuf::from))
        .find(|path| path.is_file())
}

/// Every GPU `nvidia-smi` reports now. Empty if it fails or says nothing.
pub async fn query(nvidia_smi: &Path) -> Vec<Gpu> {
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
            parse(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
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
