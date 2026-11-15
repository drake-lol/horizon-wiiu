//! Advisory file locks, which std does not offer on the Wii U (one process runs there, so every lock is
//! granted). Elsewhere these are `std::fs::File`'s own.

use std::fs::{File, TryLockError};
use std::io;

pub fn try_lock(file: &File) -> Result<(), TryLockError> {
    #[cfg(target_vendor = "wiiu")]
    {
        let _ = file;
        Ok(())
    }
    #[cfg(not(target_vendor = "wiiu"))]
    file.try_lock()
}

pub fn lock(file: &File) -> io::Result<()> {
    #[cfg(target_vendor = "wiiu")]
    {
        let _ = file;
        Ok(())
    }
    #[cfg(not(target_vendor = "wiiu"))]
    file.lock()
}

pub fn lock_shared(file: &File) -> io::Result<()> {
    #[cfg(target_vendor = "wiiu")]
    {
        let _ = file;
        Ok(())
    }
    #[cfg(not(target_vendor = "wiiu"))]
    file.lock_shared()
}

pub fn unlock(file: &File) -> io::Result<()> {
    #[cfg(target_vendor = "wiiu")]
    {
        let _ = file;
        Ok(())
    }
    #[cfg(not(target_vendor = "wiiu"))]
    file.unlock()
}
