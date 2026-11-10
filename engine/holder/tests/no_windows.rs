//! Nothing the holder does puts a window on the desktop: starting it, its
//! sessions, the console programs run in them, killing them.
//!
//! A WinEvent hook sees every window shown on the desktop while the test
//! runs, and the test counts those shown by the holder's process tree, or by
//! Windows Terminal, which takes over new console windows where it is the
//! default terminal. A second phase opens a window from a session on purpose
//! and expects the watch to see it, so a quiet first phase means something.

#![cfg(windows)]

mod common;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::time::Duration;

use common::process::{self, ancestry, snapshot};
use common::{Scratch, new_session, run, type_line, wait_for};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, DispatchMessageW, EVENT_OBJECT_SHOW, GA_ROOT, GetAncestor,
    GetWindowThreadProcessId, MSG, OBJID_WINDOW, PM_REMOVE, PeekMessageW, TranslateMessage,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

/// A top-level window that was shown, and who showed it.
#[derive(Debug)]
struct Shown {
    /// The showing process and its ancestors: (pid, exe).
    chain: Vec<(u32, String)>,
}

impl Shown {
    fn is_by(&self, holder: u32) -> bool {
        self.chain
            .iter()
            .any(|(pid, exe)| *pid == holder || exe.eq_ignore_ascii_case("WindowsTerminal.exe"))
    }
}

static SHOWN: Mutex<Vec<Shown>> = Mutex::new(Vec::new());

unsafe extern "system" fn on_show(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if object != OBJID_WINDOW || child != CHILDID_SELF as i32 {
        return;
    }
    unsafe {
        if GetAncestor(hwnd, GA_ROOT) != hwnd {
            return;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let all = snapshot();
        let chain = ancestry(&all, pid)
            .into_iter()
            .map(|pid| {
                (
                    pid,
                    all.get(&pid).map_or_else(String::new, |e| e.exe.clone()),
                )
            })
            .collect();
        SHOWN.lock().unwrap().push(Shown { chain });
    }
}

/// Collects shown windows on a thread of its own until [`Watch::finish`].
struct Watch {
    stop: std::sync::Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

impl Watch {
    fn start() -> Watch {
        SHOWN.lock().unwrap().clear();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let (ready, hooked) = channel();
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || unsafe {
            let hook = SetWinEventHook(
                EVENT_OBJECT_SHOW,
                EVENT_OBJECT_SHOW,
                std::ptr::null_mut(),
                Some(on_show),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );
            assert!(!hook.is_null(), "SetWinEventHook failed");
            ready.send(()).unwrap();
            let mut msg: MSG = std::mem::zeroed();
            while !stopping.load(Ordering::Relaxed) {
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            UnhookWinEvent(hook);
        });
        hooked.recv().unwrap();
        Watch { stop, thread }
    }

    /// Stops watching, after a moment for late windows.
    fn finish(self) -> Vec<Shown> {
        std::thread::sleep(Duration::from_secs(1));
        self.stop.store(true, Ordering::Relaxed);
        self.thread.join().unwrap();
        std::mem::take(&mut *SHOWN.lock().unwrap())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_holder_shows_no_windows() {
    let watch = Watch::start();
    let holder = Scratch::start().await;
    let mut c = holder.connect().await;
    new_session(&mut c, "ps5", "powershell.exe -NoLogo -NoProfile", "PS ").await;
    new_session(&mut c, "ps7", "pwsh.exe -NoLogo -NoProfile", "PS ").await;
    new_session(&mut c, "cmd", "cmd.exe", ">").await;
    // Console programs run from each shell, which reports `done-Windows_NT`
    // after them.
    let powershell = "cmd /c \"ver & ping -n 1 127.0.0.1\"; whoami.exe; echo \"done-$env:OS\"";
    let cmd = "ver & whoami & ping -n 1 127.0.0.1 & echo done-%OS%";
    for (session, line) in [("ps5", powershell), ("ps7", powershell), ("cmd", cmd)] {
        type_line(&mut c, session, line).await;
        wait_for(&mut c, session, "done-Windows_NT").await;
    }
    run(&mut c, &["kill-session", "-t", "ps7"]).await;
    run(&mut c, &["kill-server"]).await;
    let pid = holder.pid;
    while process::alive(pid) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let shown = watch.finish();
    let ours: Vec<&Shown> = shown.iter().filter(|s| s.is_by(pid)).collect();
    assert!(ours.is_empty(), "windows appeared: {ours:#?}");
    println!(
        "no windows from the holder; {} others shown meanwhile",
        shown.len()
    );

    // The control: a session that opens a window on purpose is seen.
    let watch = Watch::start();
    let holder = Scratch::start().await;
    let mut c = holder.connect().await;
    new_session(&mut c, "cmd", "cmd.exe", ">").await;
    type_line(&mut c, "cmd", "start \"\" /min cmd /c ping -n 3 127.0.0.1").await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let shown = watch.finish();
    let seen = shown.iter().find(|s| s.is_by(holder.pid));
    let seen = seen.unwrap_or_else(|| {
        panic!("the watch missed a window a session opened on purpose: {shown:#?}")
    });
    println!("the control's window was seen: {seen:?}");
}
