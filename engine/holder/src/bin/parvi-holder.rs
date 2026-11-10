//! The holder process. A Windows-subsystem program, so it never has a
//! console: whoever starts it, no window appears.

#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    std::process::exit(parvi_holder::serve());
    #[cfg(not(windows))]
    {
        eprintln!("parvi-holder runs on Windows only");
        std::process::exit(2);
    }
}
