//! Windows processes seen from outside: the tree, memory, and ending one.

use std::collections::HashMap;
use std::mem::{size_of, zeroed};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    TerminateProcess,
};

/// A process in a snapshot.
#[derive(Debug, Clone)]
pub struct Entry {
    pub parent: u32,
    pub exe: String,
}

/// Every process now, by pid.
pub fn snapshot() -> HashMap<u32, Entry> {
    let mut all = HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return all;
        }
        let mut entry: PROCESSENTRY32W = zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snap, &mut entry) != 0;
        while more {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
            all.insert(
                entry.th32ProcessID,
                Entry {
                    parent: entry.th32ParentProcessID,
                    exe: String::from_utf16_lossy(&entry.szExeFile[..len]),
                },
            );
            more = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    all
}

/// `pid` and its ancestors, nearest first, as far as they still run.
pub fn ancestry(all: &HashMap<u32, Entry>, pid: u32) -> Vec<u32> {
    let mut chain = vec![pid];
    let mut at = pid;
    while let Some(entry) = all.get(&at) {
        if entry.parent == 0 || chain.contains(&entry.parent) {
            break;
        }
        chain.push(entry.parent);
        at = entry.parent;
    }
    chain
}

/// The direct children of `pid`.
pub fn children(all: &HashMap<u32, Entry>, pid: u32) -> Vec<(u32, String)> {
    let mut found: Vec<(u32, String)> = all
        .iter()
        .filter(|(_, e)| e.parent == pid)
        .map(|(&child, e)| (child, e.exe.clone()))
        .collect();
    found.sort();
    found
}

fn open(pid: u32, access: u32) -> Option<HANDLE> {
    let handle = unsafe { OpenProcess(access, 0, pid) };
    (!handle.is_null()).then_some(handle)
}

/// (private bytes, working set) of a running process.
pub fn memory(pid: u32) -> Option<(usize, usize)> {
    let handle = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    unsafe {
        let mut counters: PROCESS_MEMORY_COUNTERS_EX = zeroed();
        counters.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let ok = K32GetProcessMemoryInfo(handle, (&raw mut counters).cast(), counters.cb);
        CloseHandle(handle);
        (ok != 0).then_some((counters.PrivateUsage, counters.WorkingSetSize))
    }
}

pub fn alive(pid: u32) -> bool {
    let Some(handle) = open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
        return false;
    };
    unsafe {
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE as u32
    }
}

pub fn kill(pid: u32) {
    if let Some(handle) = open(pid, PROCESS_TERMINATE) {
        unsafe {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

pub const MB: f64 = 1024.0 * 1024.0;
