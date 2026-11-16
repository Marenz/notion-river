use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

pub struct InstanceGuard {
    _file: File,
}

pub enum AcquireResult {
    Acquired(InstanceGuard),
    AlreadyRunning,
}

pub fn acquire() -> io::Result<AcquireResult> {
    let path = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("notion-river.lock");
    acquire_at(path)
}

fn acquire_at(path: PathBuf) -> io::Result<AcquireResult> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;

    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(AcquireResult::Acquired(InstanceGuard { _file: file }));
    }

    let error = io::Error::last_os_error();
    if error
        .raw_os_error()
        .is_some_and(|code| code == libc::EWOULDBLOCK || code == libc::EAGAIN)
    {
        return Ok(AcquireResult::AlreadyRunning);
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    #[test]
    fn refuses_a_second_instance() {
        let path = std::env::temp_dir().join(format!(
            "notion-river-lock-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let first = match super::acquire_at(path.clone()).unwrap() {
            super::AcquireResult::Acquired(guard) => guard,
            super::AcquireResult::AlreadyRunning => panic!("test lock was already held"),
        };

        assert!(matches!(
            super::acquire_at(path.clone()).unwrap(),
            super::AcquireResult::AlreadyRunning
        ));

        drop(first);
        std::fs::remove_file(path).unwrap();
    }
}
