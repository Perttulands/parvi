//! Fence dispatch, allow active turns to settle, then detach their observers.
use crate::{Coordinator, coordinator::Control};
use std::{
    io,
    sync::{Mutex, MutexGuard, TryLockError, atomic::Ordering},
    time::{Duration, Instant},
};

fn lock_until<T>(lock: &Mutex<T>, deadline: Instant) -> io::Result<MutexGuard<'_, T>> {
    loop {
        match lock.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(5)),
                );
            }
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "mission shutdown deadline exceeded acquiring lock",
                ));
            }
        }
    }
}

fn wake(control: &Control, deadline: Instant) -> io::Result<()> {
    if let Some(wake) = lock_until(&control.wake, deadline)?.as_ref() {
        let _ = wake.send(());
    }
    Ok(())
}

fn wait(control: &Control, deadline: Instant) -> io::Result<bool> {
    loop {
        if *lock_until(&control.done, deadline)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(5)),
        );
    }
}

impl Coordinator {
    pub fn shutdown(&self, deadline: Instant) -> Result<(), crate::Error> {
        (|| -> io::Result<()> {
            self.inner.shutting_down.store(true, Ordering::Release);
            let controls = {
                let _admission = lock_until(&self.inner.admission, deadline)?;
                let controls: Vec<_> = lock_until(&self.inner.controls, deadline)?
                    .values()
                    .cloned()
                    .collect();
                for control in &controls {
                    control.stopping.store(true, Ordering::Release);
                }
                controls
            };
            for control in &controls {
                wake(control, deadline)?;
            }
            let grace = (Instant::now() + Duration::from_secs(5)).min(deadline);
            for control in &controls {
                let _ = wait(control, grace);
            }
            for control in &controls {
                if !*lock_until(&control.done, deadline)? {
                    control.canceled.store(true, Ordering::Release);
                    wake(control, deadline)?;
                }
            }
            for control in &controls {
                if !wait(control, deadline)? {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "mission workers did not stop before shutdown deadline",
                    ));
                }
            }
            Ok(())
        })()
        .map_err(crate::error::io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lock_acquisition_obeys_the_deadline() {
        let lock = Mutex::new(false);
        let _held = lock.lock().unwrap();
        let start = Instant::now();
        assert_eq!(
            lock_until(&lock, start + Duration::from_millis(40))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
