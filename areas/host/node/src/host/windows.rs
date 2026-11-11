//! Readings from Windows itself: CPU time (`GetSystemTimes`), memory and the
//! commit charge (`GlobalMemoryStatusEx`), uptime, and what the machine is
//! from the registry. Windows keeps no load average, so there is none.

use std::ptr;

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows_sys::Win32::System::SystemInformation::{
    GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
use windows_sys::Win32::System::Threading::GetSystemTimes;

use super::cpu::CpuTimes;
use super::reading::{Load, Usage};

/// Idle, kernel and user time over every core. Kernel time includes idle.
pub fn cpu_times() -> Option<CpuTimes> {
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut idle, mut kernel, mut user) = (zero, zero, zero);
    // SAFETY: the three FILETIMEs are valid for writing.
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    Some(CpuTimes {
        total: ticks(kernel) + ticks(user),
        idle: ticks(idle),
    })
}

fn memory_status() -> Option<MEMORYSTATUSEX> {
    // SAFETY: a zeroed MEMORYSTATUSEX is valid; the call fills it once its
    // length is set.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status)
}

/// Physical memory in use: total less available.
pub fn memory() -> Option<Usage> {
    let s = memory_status()?;
    (s.ullTotalPhys > 0).then(|| Usage {
        used: s.ullTotalPhys.saturating_sub(s.ullAvailPhys),
        total: s.ullTotalPhys,
    })
}

/// Windows' nearest to swap: the commit charge against the commit limit,
/// which is memory plus the page files.
pub fn swap() -> Option<Usage> {
    let s = memory_status()?;
    Some(Usage {
        used: s.ullTotalPageFile.saturating_sub(s.ullAvailPageFile),
        total: s.ullTotalPageFile,
    })
}

pub fn load() -> Option<Load> {
    None
}

/// Seconds since boot.
pub fn uptime() -> Option<u64> {
    // SAFETY: no arguments.
    Some(unsafe { GetTickCount64() } / 1000)
}

const VERSION_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
const CPU_KEY: &str = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0";

/// Like `Windows 11 Pro 24H2`.
pub fn os_name() -> Option<String> {
    let product = registry_text(VERSION_KEY, "ProductName")?;
    let build = registry_text(VERSION_KEY, "CurrentBuildNumber");
    let display = registry_text(VERSION_KEY, "DisplayVersion");
    Some(name_os(&product, build.as_deref(), display.as_deref()))
}

/// Like `10.0.26100.4652`.
pub fn kernel() -> Option<String> {
    let major = registry_number(VERSION_KEY, "CurrentMajorVersionNumber")?;
    let minor = registry_number(VERSION_KEY, "CurrentMinorVersionNumber")?;
    let build = registry_text(VERSION_KEY, "CurrentBuildNumber")?;
    Some(match registry_number(VERSION_KEY, "UBR") {
        Some(revision) => format!("{major}.{minor}.{build}.{revision}"),
        None => format!("{major}.{minor}.{build}"),
    })
}

pub fn cpu_model() -> Option<String> {
    registry_text(CPU_KEY, "ProcessorNameString").map(|name| name.trim().to_owned())
}

/// The registry still calls Windows 11 `Windows 10` in `ProductName`; the
/// build number tells them apart (11 starts at 22000).
fn name_os(product: &str, build: Option<&str>, display: Option<&str>) -> String {
    let eleven = build
        .and_then(|b| b.parse::<u32>().ok())
        .is_some_and(|b| b >= 22000);
    let mut name = if eleven {
        product.replacen("Windows 10", "Windows 11", 1)
    } else {
        product.to_owned()
    };
    if let Some(display) = display.filter(|d| !d.is_empty()) {
        name = format!("{name} {display}");
    }
    name
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn registry_text(key: &str, value: &str) -> Option<String> {
    let (key, value) = (wide(key), wide(value));
    let mut bytes = 0u32;
    // SAFETY: the names are NUL-terminated; a null buffer asks for the size.
    let sized = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if sized != 0 || bytes == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: the buffer holds `bytes` bytes.
    let read = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if read != 0 {
        return None;
    }
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end])).filter(|text| !text.is_empty())
}

fn registry_number(key: &str, value: &str) -> Option<u32> {
    let (key, value) = (wide(key), wide(value));
    let mut number = 0u32;
    let mut bytes = size_of::<u32>() as u32;
    // SAFETY: the names are NUL-terminated and `number` holds a DWORD.
    let read = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            ptr::null_mut(),
            (&mut number as *mut u32).cast(),
            &mut bytes,
        )
    };
    (read == 0).then_some(number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_11_is_named_by_its_build() {
        assert_eq!(
            name_os("Windows 10 Pro", Some("26100"), Some("24H2")),
            "Windows 11 Pro 24H2"
        );
        assert_eq!(
            name_os("Windows 10 Pro", Some("19045"), Some("22H2")),
            "Windows 10 Pro 22H2"
        );
        assert_eq!(name_os("Windows 10 Home", None, None), "Windows 10 Home");
    }

    #[test]
    fn this_machine_names_itself() {
        assert!(os_name().is_some_and(|os| os.starts_with("Windows")));
        assert!(kernel().is_some());
        assert!(cpu_model().is_some());
        let commit = swap().unwrap();
        let memory = memory().unwrap();
        assert!(commit.total >= memory.total, "{commit:?} {memory:?}");
    }
}
